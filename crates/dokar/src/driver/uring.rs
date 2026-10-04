/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Persistent HTTP I/O on the driver's existing io_uring instance.

use super::*;
use crate::buffer::Budget;
use crate::buffer::provided::{ReceiveWindow, SHARED_SLOTS, SLOT_BYTES, SharedReceivePool};
use compio_driver::{OwnerCompletion, uring};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use uring::{cqueue, opcode, types};

const RECEIVE: u8 = 1;
const SEND: u8 = 2;
const CANCEL_RECEIVE: u8 = 3;
const CANCEL_SEND: u8 = 4;
const BATCH: usize = 256;

struct Ready {
  words: Box<[AtomicU64]>,
  wake: Waker,
}

impl Ready {
  fn mark(&self, slot: usize) {
    self.words[slot / 64].fetch_or(1 << (slot % 64), Ordering::Release);
  }
}

struct Returned {
  slot: usize,
  ready: Arc<Ready>,
}

impl Wake for Returned {
  fn wake(self: Arc<Self>) {
    self.wake_by_ref();
  }
  fn wake_by_ref(self: &Arc<Self>) {
    self.ready.mark(self.slot);
    self.ready.wake.wake_by_ref();
  }
}

struct BufferRing {
  pointer: NonNull<types::BufRingEntry>,
  tail: u16,
  mask: u16,
}

impl BufferRing {
  fn new(proactor: &mut Proactor, group: u16, entries: u16) -> io::Result<Self> {
    // Registration requires page-aligned descriptor storage; payload is separately budgeted.
    let pointer = unsafe {
      libc::mmap(
        std::ptr::null_mut(),
        4096,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_PRIVATE | libc::MAP_ANON,
        -1,
        0,
      )
    };
    if pointer == libc::MAP_FAILED {
      return Err(io::Error::last_os_error());
    }
    let pointer = NonNull::new(pointer.cast::<types::BufRingEntry>()).expect("mmap returned null");
    let mut retries = 0;
    loop {
      match unsafe { proactor.owner_register_buf_ring(pointer.as_ptr() as u64, entries, group) } {
        Ok(()) => break,
        // Closed rings release their pinned pages asynchronously, so a concurrent owner retiring
        // its ring can exhaust the pinned-page budget transiently. Bound the wait, as ring setup
        // does, rather than failing an owner that a moment later would succeed.
        Err(error) if error.kind() == io::ErrorKind::OutOfMemory && retries < 10 => {
          retries += 1;
          std::thread::sleep(Duration::from_millis(10));
        }
        Err(error) => {
          unsafe {
            libc::munmap(pointer.as_ptr().cast(), 4096);
          }
          return Err(io::Error::new(
            error.kind(),
            format!("io_uring provided buffer ring registration: {error}"),
          ));
        }
      }
    }
    Ok(Self {
      pointer,
      tail: 0,
      mask: entries - 1,
    })
  }

  fn publish(&mut self, descriptor: crate::buffer::provided::Descriptor) {
    let entry = unsafe { &mut *self.pointer.as_ptr().add(usize::from(self.tail & self.mask)) };
    entry.set_addr(descriptor.pointer as u64);
    entry.set_len(descriptor.capacity as u32);
    entry.set_bid(descriptor.id);
    self.tail = self.tail.wrapping_add(1);
    let tail = unsafe { &*types::BufRingEntry::tail(self.pointer.as_ptr()).cast::<AtomicU16>() };
    tail.store(self.tail, Ordering::Release);
  }

  fn release(self, proactor: &mut Proactor, group: u16) -> io::Result<()> {
    proactor.owner_unregister_buf_ring(group)?;
    if unsafe { libc::munmap(self.pointer.as_ptr().cast(), 4096) } == -1 {
      return Err(io::Error::last_os_error());
    }
    Ok(())
  }
}

struct SendState {
  id: u64,
  token: u64,
  buffers: SendBuffers,
  zero_copy: bool,
  retry: bool,
}

enum SendBuffers {
  Single(FrozenBuffer),
  Vectored(Vec<FrozenBuffer>),
}

impl SendBuffers {
  fn views(&self) -> &[FrozenBuffer] {
    match self {
      Self::Single(buffer) => std::slice::from_ref(buffer),
      Self::Vectored(buffers) => buffers,
    }
  }
}

struct SocketState {
  id: u64,
  _socket: SharedFd<Socket>,
  read: Rc<Cell<bool>>,
  write: Rc<Cell<bool>>,
  marker: Rc<Cell<Option<usize>>>,
  group: usize,
  window: ReceiveWindow,
  pending: VecDeque<Event>,
  waiting: bool,
  blocks_group: bool,
  owner_budget: Budget,
  receive: Option<u64>,
  paused: bool,
  closing: bool,
  eof: bool,
  receive_cancel: Option<u64>,
  send_cancel: Option<u64>,
  receive_cancel_done: bool,
  send_cancel_done: bool,
  send: Option<SendState>,
  notifications: usize,
  zero_copy_disabled: bool,
  // The boxed header is address-stable while SENDMSG is in flight. Reuse its iovec allocation.
  message: Box<libc::msghdr>,
  vectors: Vec<libc::iovec>,
}

struct Slot {
  generation: u32,
  state: Option<SocketState>,
}

struct ReceiveGroup {
  pool: SharedReceivePool,
  ring: Option<BufferRing>,
  budget: Budget,
  members: usize,
  published: usize,
  blocked_targets: usize,
  grow: bool,
  waiters: Vec<usize>,
}

pub(super) struct Persistent {
  slots: Vec<Slot>,
  free: Vec<usize>,
  ids: IntMap<u64, usize>,
  sends: IntMap<u64, usize>,
  ready: Arc<Ready>,
  group_ready: Arc<Ready>,
  groups: Vec<Option<ReceiveGroup>>,
  free_groups: Vec<usize>,
  budget_groups: IntMap<usize, usize>,
  completions: Vec<OwnerCompletion>,
  active: usize,
  limit: usize,
  ready_cursor: usize,
  stats: PersistentStats,
  zero_copy: super::send_zc::Sends,
  send_zc: bool,
  sendmsg_zc: bool,
}

impl Persistent {
  pub(super) fn new(mut proactor: Proactor, limit: usize) -> io::Result<(Proactor, Self)> {
    let capacity = limit
      .checked_mul(6)
      .and_then(|n| n.checked_add(8))
      .ok_or(io::ErrorKind::InvalidInput)?;
    proactor.owner_init(capacity)?;
    proactor.owner_register_files_sparse(limit as u32).map_err(|error| {
      io::Error::new(
        error.kind(),
        format!("io_uring sparse fixed-file registration: {error}"),
      )
    })?;
    proactor = probe(proactor)?;
    let supported = proactor.owner_probe().ok();
    let send_zc = supported
      .as_ref()
      .is_some_and(|probe| probe.is_supported(opcode::SendZc::CODE));
    let sendmsg_zc = supported
      .as_ref()
      .is_some_and(|probe| probe.is_supported(opcode::SendMsgZc::CODE));
    let persistent = Self {
      slots: Vec::new(),
      free: Vec::new(),
      ids: IntMap::default(),
      sends: IntMap::default(),
      ready: Arc::new(Ready {
        words: (0..limit.div_ceil(64)).map(|_| AtomicU64::new(0)).collect(),
        wake: proactor.waker(),
      }),
      group_ready: Arc::new(Ready {
        words: (0..limit.div_ceil(64)).map(|_| AtomicU64::new(0)).collect(),
        wake: proactor.waker(),
      }),
      groups: Vec::new(),
      free_groups: Vec::new(),
      budget_groups: IntMap::default(),
      completions: Vec::with_capacity(BATCH),
      active: 0,
      limit,
      ready_cursor: 0,
      stats: PersistentStats::default(),
      zero_copy: super::send_zc::Sends::default(),
      send_zc,
      sendmsg_zc,
    };
    Ok((proactor, persistent))
  }

  pub(super) fn has_ready(&self) -> bool {
    self
      .ready
      .words
      .iter()
      .chain(self.group_ready.words[..self.groups.len().div_ceil(64)].iter())
      .any(|word| word.load(Ordering::Acquire) != 0)
  }

  pub(super) fn stats(&self) -> PersistentStats {
    PersistentStats {
      zero_copy_supported: self.send_zc || self.sendmsg_zc,
      zero_copy_pending_bytes: self.zero_copy.bytes(),
      ..self.stats
    }
  }

  pub(super) fn transient_outstanding(&self) -> usize {
    self.sends.len()
  }

  pub(super) fn outstanding(&self) -> usize {
    self.active + self.sends.len()
  }
  pub(super) fn is_empty(&self) -> bool {
    self.active == 0
  }

  pub(super) fn enable(
    &mut self,
    proactor: &mut Proactor,
    connection: &Connection,
    id: u64,
    budget: Budget,
    capacity: usize,
    window: usize,
  ) -> io::Result<()> {
    if capacity != SLOT_BYTES {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let index = if let Some(index) = self.free.pop() {
      index
    } else {
      if self.slots.len() >= self.limit {
        return Err(io::ErrorKind::WouldBlock.into());
      }
      self.slots.push(Slot {
        generation: 0,
        state: None,
      });
      self.slots.len() - 1
    };
    let initialized = (|| {
      let wake = Waker::from(Arc::new(Returned {
        slot: index,
        ready: self.ready.clone(),
      }));
      let window = ReceiveWindow::new(window, wake)?;
      proactor.owner_update_files(index as u32, &[connection.socket.as_raw_fd()])?;
      let group = match self.acquire_group(proactor, budget.clone()) {
        Ok(group) => group,
        Err(error) => {
          proactor.owner_update_files(index as u32, &[-1])?;
          return Err(error);
        }
      };
      Ok((window, group))
    })();
    let (window, group) = match initialized {
      Ok(value) => value,
      Err(error) => {
        self.free.push(index);
        return Err(error);
      }
    };
    self.slots[index].state = Some(SocketState {
      id,
      _socket: connection.socket.clone(),
      read: connection.read.clone(),
      write: connection.write.clone(),
      marker: connection.persistent.clone(),
      group,
      window,
      pending: VecDeque::new(),
      waiting: false,
      blocks_group: false,
      owner_budget: budget,
      receive: None,
      paused: false,
      closing: false,
      eof: false,
      receive_cancel: None,
      send_cancel: None,
      receive_cancel_done: false,
      send_cancel_done: false,
      send: None,
      notifications: 0,
      zero_copy_disabled: false,
      message: Box::new(unsafe { std::mem::zeroed() }),
      vectors: Vec::with_capacity(8),
    });
    connection.persistent.set(Some(index));
    connection.read.set(true);
    self.ids.insert(id, index);
    self.active += 1;
    self.stats.active_sockets_peak = self.stats.active_sockets_peak.max(self.active);
    self.stats.owner_used_high_water_bytes = self
      .stats
      .owner_used_high_water_bytes
      .max(self.slots[index].state.as_ref().unwrap().owner_budget.used());
    self.ready.mark(index);
    // Submission is serviced by poll; no partial success escapes into caller cleanup.
    Ok(())
  }

  fn acquire_group(&mut self, proactor: &mut Proactor, budget: Budget) -> io::Result<usize> {
    if let Some(&index) = self.budget_groups.get(&budget.identity()) {
      self.groups[index].as_mut().unwrap().members += 1;
      return Ok(index);
    }
    let index = self.free_groups.pop().unwrap_or_else(|| {
      self.groups.push(None);
      self.groups.len() - 1
    });
    let created = (|| {
      let wake = Waker::from(Arc::new(Returned {
        slot: index,
        ready: self.group_ready.clone(),
      }));
      let pool = SharedReceivePool::new(budget.clone(), wake)?;
      let ring = BufferRing::new(proactor, index as u16, SHARED_SLOTS as u16)?;
      Ok(ReceiveGroup {
        pool,
        ring: Some(ring),
        budget: budget.clone(),
        members: 1,
        published: 0,
        blocked_targets: 0,
        grow: false,
        waiters: Vec::new(),
      })
    })();
    match created {
      Ok(group) => {
        self.groups[index] = Some(group);
        self.budget_groups.insert(budget.identity(), index);
        self.group_ready.mark(index);
        Ok(index)
      }
      Err(error) => {
        self.free_groups.push(index);
        Err(error)
      }
    }
  }

  fn service_groups(&mut self) -> io::Result<()> {
    for (word_index, word) in self.group_ready.words[..self.groups.len().div_ceil(64)]
      .iter()
      .enumerate()
    {
      let mut bits = word.swap(0, Ordering::AcqRel);
      while bits != 0 {
        let index = word_index * 64 + bits.trailing_zeros() as usize;
        bits &= bits - 1;
        let Some(group) = self.groups.get_mut(index).and_then(Option::as_mut) else {
          continue;
        };
        // Retire credit-blocked targets before replenishing: one shared ring bounds read-ahead.
        if group.blocked_targets != 0 {
          continue;
        }
        if group.grow {
          match group.pool.grow() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {}
            Err(error) => return Err(error),
          }
          group.grow = false;
        }
        for id in 0..group.pool.len() as u16 {
          match group.pool.publish(id) {
            Ok(descriptor) => {
              group.ring.as_mut().unwrap().publish(descriptor);
              group.published += 1;
              self.stats.buffers_published += 1;
            }
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::OutOfMemory) => {}
            Err(error) => return Err(error),
          }
        }
        self.stats.owner_used_high_water_bytes = self.stats.owner_used_high_water_bytes.max(group.budget.used());
        if group.published != 0 {
          for socket in group.waiters.drain(..) {
            if let Some(state) = self.slots[socket].state.as_mut()
              && state.group == index
              && state.waiting
            {
              state.waiting = false;
              self.ready.mark(socket);
            }
          }
        }
      }
    }
    Ok(())
  }

  pub(super) fn pause(&mut self, id: u64, paused: bool) -> io::Result<()> {
    let &index = self.ids.get(&id).ok_or(io::ErrorKind::NotFound)?;
    let state = self.slots[index].state.as_mut().expect("live receive id");
    if state.closing {
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    if paused && !state.paused {
      self.stats.admission_pauses += 1;
    }
    state.paused = paused;
    self.ready.mark(index);
    Ok(())
  }

  pub(super) fn cancel(&mut self, id: u64) -> bool {
    if let Some(&index) = self.ids.get(&id) {
      let state = self.slots[index].state.as_mut().expect("live receive id");
      state.closing = true;
      self.ready.mark(index);
      true
    } else if let Some(&index) = self.sends.get(&id) {
      // Cancelling a write closes the lane: its physical completion still returns the views.
      self.slots[index].state.as_mut().expect("live send id").closing = true;
      self.ready.mark(index);
      true
    } else {
      false
    }
  }

  pub(super) fn close_all(&mut self) {
    for (index, slot) in self.slots.iter_mut().enumerate() {
      if let Some(state) = &mut slot.state {
        state.closing = true;
        self.ready.mark(index);
      }
    }
  }

  fn token(slot: &mut Slot, index: usize, kind: u8) -> io::Result<u64> {
    // Reserve two identities for retiring a live receive and send before retiring the slot.
    if kind < CANCEL_RECEIVE && slot.generation >= u32::MAX - 2 {
      return Err(io::ErrorKind::OutOfMemory.into());
    }
    slot.generation = slot.generation.checked_add(1).ok_or(io::ErrorKind::OutOfMemory)?;
    Ok((u64::from(slot.generation) << 24) | ((index as u64) << 8) | u64::from(kind))
  }

  pub(super) fn send(
    &mut self,
    proactor: &mut Proactor,
    index: usize,
    id: u64,
    buffer: FrozenBuffer,
  ) -> Result<(), (io::Error, FrozenBuffer)> {
    match self.submit_send(proactor, index, id, SendBuffers::Single(buffer)) {
      Ok(()) => Ok(()),
      Err((error, SendBuffers::Single(buffer))) => Err((error, buffer)),
      Err(_) => unreachable!(),
    }
  }

  pub(super) fn send_vectored(
    &mut self,
    proactor: &mut Proactor,
    index: usize,
    id: u64,
    buffers: Vec<FrozenBuffer>,
  ) -> Result<(), (io::Error, Vec<FrozenBuffer>)> {
    match self.submit_send(proactor, index, id, SendBuffers::Vectored(buffers)) {
      Ok(()) => Ok(()),
      Err((error, SendBuffers::Vectored(buffers))) => Err((error, buffers)),
      Err(_) => unreachable!(),
    }
  }

  fn submit_send(
    &mut self,
    proactor: &mut Proactor,
    index: usize,
    id: u64,
    buffers: SendBuffers,
  ) -> Result<(), (io::Error, SendBuffers)> {
    let slot = &mut self.slots[index];
    let token = match Self::token(slot, index, SEND) {
      Ok(token) => token,
      Err(error) => {
        slot.state.as_mut().expect("connection slot is live").closing = true;
        self.ready.mark(index);
        return Err((error, buffers));
      }
    };
    let state = slot.state.as_mut().expect("connection slot is live");
    if state.closing || state.send.is_some() {
      return Err((io::ErrorKind::BrokenPipe.into(), buffers));
    }
    self.stats.owner_used_high_water_bytes = self.stats.owner_used_high_water_bytes.max(state.owner_budget.used());
    let views = buffers.views();
    let supported = if views.len() == 1 {
      self.send_zc
    } else {
      self.sendmsg_zc
    };
    let zero_copy = supported
      && !state.zero_copy_disabled
      && views
        .iter()
        .fold(0usize, |length, view| length.saturating_add(view.as_ref().len()))
        >= super::send_zc::THRESHOLD
      && self.zero_copy.retain(token, views);
    let entry = Self::send_entry(state, index, views, zero_copy);
    if let Err(error) = unsafe { proactor.owner_push(entry, token) } {
      if zero_copy {
        self.zero_copy.abandon(token);
      }
      return Err((error, buffers));
    }
    if zero_copy {
      state.notifications += 1;
      self.stats.zero_copy_sends += 1;
    }
    state.send = Some(SendState {
      id,
      token,
      buffers,
      zero_copy,
      retry: false,
    });
    state.write.set(true);
    self.sends.insert(id, index);
    Ok(())
  }

  fn send_entry(
    state: &mut SocketState,
    index: usize,
    views: &[FrozenBuffer],
    zero_copy: bool,
  ) -> uring::squeue::Entry {
    // IORING_SEND_ZC_REPORT_USAGE; notification bit 31 reports a copied fallback.
    const REPORT_USAGE: u16 = 1 << 3;
    if let [buffer] = views
      && let Ok(length) = u32::try_from(buffer.as_ref().len())
    {
      if zero_copy {
        opcode::SendZc::new(types::Fixed(index as u32), buffer.as_ref().as_ptr(), length)
          .flags(libc::MSG_NOSIGNAL)
          .zc_flags(REPORT_USAGE)
          .build()
      } else {
        opcode::Send::new(types::Fixed(index as u32), buffer.as_ref().as_ptr(), length)
          .flags(libc::MSG_NOSIGNAL)
          .build()
      }
    } else {
      state.vectors.clear();
      state.vectors.extend(views.iter().map(|buffer| libc::iovec {
        iov_base: buffer.as_ref().as_ptr().cast_mut().cast(),
        iov_len: buffer.as_ref().len(),
      }));
      state.message.msg_iov = state.vectors.as_mut_ptr();
      state.message.msg_iovlen = state.vectors.len();
      if zero_copy {
        opcode::SendMsgZc::new(types::Fixed(index as u32), &*state.message)
          .flags(libc::MSG_NOSIGNAL as u32)
          .ioprio(REPORT_USAGE)
          .build()
      } else {
        opcode::SendMsg::new(types::Fixed(index as u32), &*state.message)
          .flags(libc::MSG_NOSIGNAL as u32)
          .build()
      }
    }
  }

  fn finish_send(send: SendState, result: io::Result<usize>, output: &mut VecDeque<Event>) {
    match send.buffers {
      SendBuffers::Single(buffer) => output.push_back(Event::Sent {
        id: send.id,
        result,
        buffer,
      }),
      SendBuffers::Vectored(buffers) => output.push_back(Event::SentVectored {
        id: send.id,
        result,
        buffers,
      }),
    }
  }

  fn retry_send(&mut self, index: usize, proactor: &mut Proactor, output: &mut VecDeque<Event>) -> io::Result<()> {
    let slot = &mut self.slots[index];
    let state = slot.state.as_mut().expect("ready live slot");
    if !state.send.as_ref().is_some_and(|send| send.retry) {
      return Ok(());
    }
    let mut send = state.send.take().unwrap();
    let result = if state.closing {
      Err(io::Error::from_raw_os_error(libc::ECANCELED))
    } else {
      Self::token(slot, index, SEND).and_then(|token| {
        send.token = token;
        let state = slot.state.as_mut().unwrap();
        let entry = Self::send_entry(state, index, send.buffers.views(), false);
        unsafe { proactor.owner_push(entry, token) }
      })
    };
    let state = slot.state.as_mut().unwrap();
    match result {
      Ok(()) => {
        send.retry = false;
        state.send = Some(send);
      }
      Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
        state.send = Some(send);
        self.ready.mark(index);
      }
      Err(error) => {
        self.sends.remove(&send.id);
        state.write.set(false);
        Self::finish_send(send, Err(error), output);
      }
    }
    Ok(())
  }

  pub(super) fn prepare(&mut self, proactor: &mut Proactor, output: &mut VecDeque<Event>) -> io::Result<()> {
    self.service_ready(proactor, output)
  }

  pub(super) fn drain(&mut self, proactor: &mut Proactor, output: &mut VecDeque<Event>) -> io::Result<()> {
    proactor.owner_drain(&mut self.completions, BATCH);
    // Move out the retained vector so completion handling can mutate the socket slab.
    let mut completions = std::mem::take(&mut self.completions);
    for completion in completions.drain(..) {
      self.complete(completion, output)?;
    }
    self.completions = completions;
    Ok(())
  }

  fn service_ready(&mut self, proactor: &mut Proactor, output: &mut VecDeque<Event>) -> io::Result<()> {
    self.service_groups()?;
    // A fixed bitmap bounds foreign-thread return notifications, including repeated releases.
    let mut remaining = BATCH;
    for offset in 0..self.ready.words.len() {
      let word = (self.ready_cursor + offset) % self.ready.words.len();
      let mut bits = self.ready.words[word].swap(0, Ordering::AcqRel);
      while bits != 0 && remaining != 0 {
        let bit = bits.trailing_zeros() as usize;
        bits &= bits - 1;
        remaining -= 1;
        let index = word * 64 + bit;
        if index >= self.slots.len() || self.slots[index].state.is_none() {
          continue;
        }
        self.service(index, proactor, output)?;
      }
      if bits != 0 {
        self.ready.words[word].fetch_or(bits, Ordering::Release);
      }
      if remaining == 0 {
        self.ready_cursor = (word + 1) % self.ready.words.len();
        break;
      }
    }
    Ok(())
  }

  fn service(&mut self, index: usize, proactor: &mut Proactor, output: &mut VecDeque<Event>) -> io::Result<()> {
    self.retry_send(index, proactor, output)?;
    let slot = &mut self.slots[index];
    let state = slot.state.as_mut().expect("ready live slot");
    self.stats.owner_used_high_water_bytes = self.stats.owner_used_high_water_bytes.max(state.owner_budget.used());
    if state.receive.is_none() && state.receive_cancel_done {
      state.receive_cancel = None;
      state.receive_cancel_done = false;
    }
    if state.send.is_none() && state.send_cancel_done {
      state.send_cancel = None;
      state.send_cancel_done = false;
    }
    if state.closing {
      state.pending.clear();
    } else if !state.paused {
      while let Some(event) = state.pending.front_mut() {
        if let Event::PersistentReceived {
          buffer: Some(buffer),
          credit,
          ..
        } = event
        {
          let Some(reserved) = state.window.attach(buffer) else {
            break;
          };
          *credit = Some(reserved);
        }
        output.push_back(state.pending.pop_front().unwrap());
      }
    }
    let credit_paused = !state.window.available() || !state.pending.is_empty();
    let blocks_group = credit_paused && state.receive.is_some();
    if state.blocks_group != blocks_group {
      let group = self.groups[state.group].as_mut().unwrap();
      if blocks_group {
        group.blocked_targets += 1;
      } else {
        group.blocked_targets -= 1;
      }
      state.blocks_group = blocks_group;
      self.group_ready.mark(state.group);
    }
    if (state.closing || state.paused || credit_paused)
      && let Some(target) = state.receive.filter(|_| state.receive_cancel.is_none())
    {
      let token = Self::token(slot, index, CANCEL_RECEIVE)?;
      match unsafe { proactor.owner_push(opcode::AsyncCancel::new((target << 1) | 1).build(), token) } {
        Ok(()) => slot.state.as_mut().unwrap().receive_cancel = Some(token),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.ready.mark(index),
        Err(error) => return Err(error),
      }
    }
    let state = slot.state.as_mut().unwrap();
    if state.closing
      && let Some(target) = state
        .send
        .as_ref()
        .filter(|_| state.send_cancel.is_none())
        .map(|send| send.token)
    {
      let token = Self::token(slot, index, CANCEL_SEND)?;
      match unsafe { proactor.owner_push(opcode::AsyncCancel::new((target << 1) | 1).build(), token) } {
        Ok(()) => slot.state.as_mut().unwrap().send_cancel = Some(token),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.ready.mark(index),
        Err(error) => return Err(error),
      }
    }
    let state = slot.state.as_mut().unwrap();
    if state.closing
      && state.receive.is_none()
      && state.send.is_none()
      && state.receive_cancel.is_none()
      && state.send_cancel.is_none()
      && state.notifications == 0
    {
      // The group's final connection retires only after its target and cancel completions.
      let group_index = state.group;
      let group = self.groups[group_index].as_mut().unwrap();
      if state.waiting {
        group.waiters.retain(|waiting| *waiting != index);
      }
      group.members -= 1;
      if group.members == 0 {
        group.ring.take().unwrap().release(proactor, group_index as u16)?;
        unsafe {
          group.pool.retire_published();
        }
        self.budget_groups.remove(&group.budget.identity());
        self.groups[group_index] = None;
        self.free_groups.push(group_index);
      }
      proactor.owner_update_files(index as u32, &[-1])?;
      let state = slot.state.take().unwrap();
      self.ids.remove(&state.id);
      state.marker.set(None);
      state.read.set(false);
      state.write.set(false);
      output.push_back(Event::PersistentRetired { id: state.id });
      self.active -= 1;
      if slot.generation < u32::MAX - 2 {
        self.free.push(index);
      }
      return Ok(());
    }
    if state.closing || state.paused || credit_paused || state.eof || state.receive_cancel.is_some() {
      return Ok(());
    }
    if state.receive.is_none() {
      let group = self.groups[state.group].as_mut().unwrap();
      if group.published == 0 {
        if !state.waiting {
          group.waiters.push(index);
          state.waiting = true;
        }
        return Ok(());
      }
      let token = match Self::token(slot, index, RECEIVE) {
        Ok(token) => token,
        Err(error) => {
          let state = slot.state.as_mut().unwrap();
          state.closing = true;
          output.push_back(Event::PersistentReceived {
            id: state.id,
            result: Err(error),
            buffer: None,
            credit: None,
            terminal: true,
          });
          self.ready.mark(index);
          return Ok(());
        }
      };
      let group = slot.state.as_ref().unwrap().group;
      let entry = opcode::RecvMulti::new(types::Fixed(index as u32), group as u16).build();
      match unsafe { proactor.owner_push(entry, token) } {
        Ok(()) => {
          slot.state.as_mut().unwrap().receive = Some(token);
          self.stats.receive_registrations += 1;
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.ready.mark(index),
        Err(error) => return Err(error),
      }
    }
    Ok(())
  }

  fn complete(&mut self, completion: OwnerCompletion, output: &mut VecDeque<Event>) -> io::Result<()> {
    let index = ((completion.token >> 8) & 0xffff) as usize;
    let kind = completion.token as u8;
    let slot = self.slots.get_mut(index).ok_or(io::ErrorKind::InvalidData)?;
    let state = slot.state.as_mut().ok_or(io::ErrorKind::InvalidData)?;
    match kind {
      RECEIVE => {
        if state.receive != Some(completion.token) {
          return Err(io::ErrorKind::InvalidData.into());
        }
        self.stats.receive_completions += 1;
        let terminal = !cqueue::more(completion.flags);
        if terminal {
          self.stats.terminal_completions += 1;
        }
        let selected = cqueue::buffer_select(completion.flags);
        let group = self.groups[state.group].as_mut().unwrap();
        let buffer = if let Some(id) = selected {
          let length = usize::try_from(completion.result).unwrap_or(0);
          let buffer = unsafe { group.pool.complete(id, length)? };
          group.published = group.published.checked_sub(1).ok_or(io::ErrorKind::InvalidData)?;
          Some(buffer)
        } else {
          if completion.result > 0 {
            return Err(io::ErrorKind::InvalidData.into());
          }
          None
        };
        if terminal {
          state.receive = None;
        }
        let exhausted = completion.result == -libc::ENOBUFS;
        if exhausted {
          self.stats.buffer_exhaustions += 1;
          group.grow = true;
        }
        let cancelled = completion.result == -libc::ECANCELED;
        if !state.closing && !exhausted && !cancelled {
          let result = if completion.result < 0 {
            Err(io::Error::from_raw_os_error(-completion.result))
          } else {
            Ok(completion.result as usize)
          };
          if completion.result <= 0 {
            state.eof = true;
          }
          let mut event = Event::PersistentReceived {
            id: state.id,
            result,
            buffer,
            credit: None,
            terminal,
          };
          let deliver = !state.paused
            && state.pending.is_empty()
            && match &mut event {
              Event::PersistentReceived {
                buffer: Some(buffer),
                credit,
                ..
              } => {
                *credit = state.window.attach(buffer);
                credit.is_some()
              }
              _ => true,
            };
          if deliver {
            output.push_back(event);
          } else {
            state.pending.push_back(event);
          }
        }
        let blocks_group = (!state.window.available() || !state.pending.is_empty()) && state.receive.is_some();
        if state.blocks_group != blocks_group {
          if blocks_group {
            group.blocked_targets += 1;
          } else {
            group.blocked_targets -= 1;
          }
          state.blocks_group = blocks_group;
        }
        self.group_ready.mark(state.group);
      }
      SEND => {
        if cqueue::notif(completion.flags) {
          if cqueue::more(completion.flags) {
            return Err(io::ErrorKind::InvalidData.into());
          }
          if self.zero_copy.notification(completion.token)? {
            state.notifications -= 1;
          }
          self.stats.zero_copy_notifications += 1;
          if completion.result as u32 & (1 << 31) != 0 {
            self.stats.zero_copy_copied += 1;
            // Loopback and unsupported paths should not repeatedly pay for copied notifications.
            state.zero_copy_disabled = true;
          }
          self.ready.mark(index);
          return Ok(());
        }
        if state.send.as_ref().map(|send| send.token) != Some(completion.token) {
          return Err(io::ErrorKind::InvalidData.into());
        }
        let more = cqueue::more(completion.flags);
        let zero_copy = state.send.as_ref().unwrap().zero_copy;
        if zero_copy {
          if self.zero_copy.initial(completion.token, more)? {
            state.notifications -= 1;
          }
        } else if more {
          return Err(io::ErrorKind::InvalidData.into());
        }
        let mut send = state.send.take().unwrap();
        if zero_copy
          && matches!(
            -completion.result,
            libc::EINVAL | libc::EOPNOTSUPP | libc::ENOSYS | libc::ENOMEM | libc::ENOBUFS | libc::EPERM
          )
        {
          self.stats.zero_copy_fallbacks += 1;
          state.zero_copy_disabled |= !matches!(-completion.result, libc::ENOMEM | libc::ENOBUFS);
          send.zero_copy = false;
          send.retry = true;
          state.send = Some(send);
          self.ready.mark(index);
          return Ok(());
        }
        self.stats.send_completions += 1;
        self.sends.remove(&send.id);
        state.write.set(false);
        let result = if completion.result < 0 {
          Err(io::Error::from_raw_os_error(-completion.result))
        } else {
          Ok(completion.result as usize)
        };
        Self::finish_send(send, result, output);
      }
      CANCEL_RECEIVE => {
        if state.receive_cancel != Some(completion.token) {
          return Err(io::ErrorKind::InvalidData.into());
        }
        state.receive_cancel_done = true;
        // ENOENT races a target CQE; the target still owns all receive resources.
        if completion.result < 0 && completion.result != -libc::ENOENT && completion.result != -libc::EALREADY {
          return Err(io::Error::from_raw_os_error(-completion.result));
        }
      }
      CANCEL_SEND => {
        if state.send_cancel != Some(completion.token) {
          return Err(io::ErrorKind::InvalidData.into());
        }
        state.send_cancel_done = true;
        if completion.result < 0 && completion.result != -libc::ENOENT && completion.result != -libc::EALREADY {
          return Err(io::Error::from_raw_os_error(-completion.result));
        }
      }
      _ => return Err(io::ErrorKind::InvalidData.into()),
    }
    self.ready.mark(index);
    Ok(())
  }
}

fn probe(mut proactor: Proactor) -> io::Result<Proactor> {
  let (receiver, peer) = std::os::unix::net::UnixStream::pair()?;
  let budget = Budget::new(2 * SLOT_BYTES);
  let mut pool = SharedReceivePool::new(budget, proactor.waker())?;
  let mut ring = BufferRing::new(&mut proactor, 0, 2)?;
  let outcome = (|| {
    proactor.owner_update_files(0, &[receiver.as_raw_fd()])?;
    for id in 0..2 {
      ring.publish(pool.publish(id)?);
    }
    // A real MORE+BUFFER completion verifies modifiers that opcode probing cannot establish.
    let entry = opcode::RecvMulti::new(types::Fixed(0), 0).build();
    unsafe {
      proactor.owner_push(entry, 0)?;
    }
    use std::io::Write;
    (&peer).write_all(b"p")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut completions = Vec::with_capacity(4);
    let mut selected = false;
    let mut terminal = false;
    let mut cancelled = false;
    let mut cancel_submitted = false;
    while !terminal || !cancelled {
      proactor.owner_progress()?;
      proactor.owner_drain(&mut completions, 4);
      for completion in completions.drain(..) {
        if completion.token == 0 {
          if completion.result == 1 && cqueue::more(completion.flags) {
            let id = cqueue::buffer_select(completion.flags).ok_or(io::ErrorKind::InvalidData)?;
            let lease = unsafe { pool.complete(id, 1)? };
            drop(lease);
            selected = true;
          } else if completion.result == -libc::ECANCELED && !cqueue::more(completion.flags) {
            terminal = true;
          } else {
            return Err(io::Error::new(
              io::ErrorKind::Unsupported,
              format!(
                "io_uring RecvMulti fixed-file/buffer-selection probe: result={}, flags={}",
                completion.result, completion.flags
              ),
            ));
          }
        } else if completion.token == 1 {
          if completion.result < 0 && completion.result != -libc::ENOENT {
            return Err(io::Error::from_raw_os_error(-completion.result));
          }
          cancelled = true;
        } else {
          return Err(io::ErrorKind::InvalidData.into());
        }
      }
      if selected && !cancel_submitted {
        // Owner token zero is encoded by CompIO as user_data 1.
        unsafe {
          proactor.owner_push(opcode::AsyncCancel::new(1).build(), 1)?;
        }
        cancel_submitted = true;
      }
      if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
      }
      std::thread::yield_now();
    }
    Ok(())
  })();
  if let Err(error) = outcome {
    // Closing a ring can retire asynchronously. Retain its mapping and published backing
    // on an unverified startup failure; a timeout never proves kernel memory is releasable.
    drop(proactor);
    return Err(io::Error::new(
      error.kind(),
      format!("io_uring persistent receive capability probe failed: {error}"),
    ));
  }
  ring.release(&mut proactor, 0)?;
  unsafe {
    pool.retire_published();
  }
  proactor.owner_update_files(0, &[-1])?;
  Ok(proactor)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn zero_copy_rejection_retries_copy_and_close_waits_for_old_notification() {
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    for (error, more) in [libc::ENOMEM, libc::ENOBUFS, libc::EOPNOTSUPP, libc::EPERM]
      .into_iter()
      .flat_map(|error| [false, true].map(|more| (error, more)))
    {
      let mut driver = Driver::new(Backend::IoUring, 8).unwrap();
      let listener = TcpListener::bind("127.0.0.1:0").unwrap();
      let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
      let (mut peer, _) = listener.accept().unwrap();
      peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
      let connection = driver.attach(client.into()).unwrap();
      let budget = Budget::new(131072);
      let receive = driver
        .enable_persistent_receive(&connection, budget.clone(), SLOT_BYTES, 2 * SLOT_BYTES)
        .unwrap();
      let index = connection.persistent.get().unwrap();
      let mut buffer = Buffer::new(SLOT_BYTES, budget.clone()).unwrap();
      buffer.write(0, b"fallback").unwrap();
      let buffer = buffer.freeze();
      let persistent = driver.persistent.as_mut().unwrap();
      let token = Persistent::token(&mut persistent.slots[index], index, SEND).unwrap();
      assert!(persistent.zero_copy.retain(token, std::slice::from_ref(&buffer)));
      let state = persistent.slots[index].state.as_mut().unwrap();
      state.notifications = 1;
      state.write.set(true);
      // Inject a rejected initial result only: no send SQE references these bytes yet.
      state.send = Some(SendState {
        id: u64::MAX,
        token,
        buffers: SendBuffers::Single(buffer),
        zero_copy: true,
        retry: false,
      });
      persistent.sends.insert(u64::MAX, index);
      let mut output = VecDeque::new();
      persistent
        .complete(
          OwnerCompletion {
            token,
            result: -error,
            flags: if more { 1 << 1 } else { 0 },
          },
          &mut output,
        )
        .unwrap();
      assert!(output.is_empty());
      assert_eq!(
        persistent.slots[index].state.as_ref().unwrap().zero_copy_disabled,
        !matches!(error, libc::ENOMEM | libc::ENOBUFS)
      );
      let deadline = Instant::now() + Duration::from_secs(5);
      loop {
        assert!(Instant::now() < deadline);
        let events = driver.poll(Duration::from_millis(10), 8).unwrap();
        if let Some(Event::Sent { id, result, .. }) = events.first() {
          assert_eq!(*id, u64::MAX);
          assert_eq!(*result.as_ref().unwrap(), 8);
          break;
        }
        assert!(events.is_empty());
      }
      let mut bytes = [0; 8];
      peer.read_exact(&mut bytes).unwrap();
      assert_eq!(&bytes, b"fallback");
      assert!(driver.cancel(receive));
      if more {
        assert!(!driver.try_shutdown(Duration::from_millis(10)).unwrap());
        assert_eq!(driver.persistent_stats().unwrap().zero_copy_pending_bytes, SLOT_BYTES);
        driver
          .persistent
          .as_mut()
          .unwrap()
          .complete(
            OwnerCompletion {
              token,
              result: 0,
              flags: 1 << 3,
            },
            &mut output,
          )
          .unwrap();
      }
      assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
      assert_eq!(driver.persistent_stats().unwrap().zero_copy_fallbacks, 1);
      assert_eq!(budget.used(), 0);
    }
  }

  #[test]
  fn generation_tokens_do_not_alias_slots_kinds_or_owner_namespace() {
    let mut slot = Slot {
      generation: 0,
      state: None,
    };
    let receive = Persistent::token(&mut slot, 65535, RECEIVE).unwrap();
    let cancel = Persistent::token(&mut slot, 65535, CANCEL_RECEIVE).unwrap();
    assert_ne!(receive, cancel);
    assert_eq!((receive >> 8) & 0xffff, 65535);
    assert_eq!(receive as u8, RECEIVE);
    slot.generation = u32::MAX - 1;
    assert!(Persistent::token(&mut slot, 65535, SEND).is_err());
    let final_token = Persistent::token(&mut slot, 65535, CANCEL_SEND).unwrap();
    assert!(final_token < u64::MAX >> 1);
    assert!(Persistent::token(&mut slot, 65535, RECEIVE).is_err());
    assert_eq!(slot.generation, u32::MAX);
  }
}
