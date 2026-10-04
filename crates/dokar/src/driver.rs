/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Owner-thread completion driver. Wakers notify only; they never enter a consumer runtime.

use nohash_hasher::IntMap;
use std::cell::Cell;
use std::collections::VecDeque;
use std::io;
use std::mem::ManuallyDrop;
use std::net::{Shutdown, SocketAddr};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use compio_buf::{BufResult, IntoInner, SetLen};
use compio_driver::{AsRawFd, Cancel, DriverType, Key, Proactor, PushEntry, SharedFd, op};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::buffer::{Budget, Buffer, FrozenBuffer, ReceiveCredit};

#[cfg(any(target_os = "linux", test))]
mod send_zc;
#[cfg(target_os = "linux")]
mod uring;

/// Requested or selected native I/O backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Backend {
  /// Select an available native backend.
  Auto = 0,
  /// Polling (epoll on Linux, kqueue on macOS).
  Polling = 1,
  /// Linux io_uring.
  IoUring = 2,
  /// Windows completion ports.
  Iocp = 3,
}

/// Owner-local structural diagnostics for persistent I/O.
#[derive(Clone, Copy, Debug, Default)]
pub struct PersistentStats {
  pub receive_registrations: u64,
  pub receive_completions: u64,
  pub terminal_completions: u64,
  pub buffer_exhaustions: u64,
  pub buffers_published: u64,
  pub send_completions: u64,
  pub zero_copy_supported: bool,
  pub zero_copy_sends: u64,
  pub zero_copy_notifications: u64,
  pub zero_copy_copied: u64,
  pub zero_copy_fallbacks: u64,
  pub zero_copy_pending_bytes: usize,
  pub active_sockets_peak: usize,
  pub admission_pauses: u64,
  /// Maximum observed total owner-budget usage, including non-pool storage.
  pub owner_used_high_water_bytes: usize,
}

/// Socket registered with exactly one driver.
pub struct Connection {
  socket: SharedFd<Socket>,
  owner: Rc<()>,
  read: Rc<Cell<bool>>,
  write: Rc<Cell<bool>>,
  #[cfg(target_os = "linux")]
  persistent: Rc<Cell<Option<usize>>>,
}

impl Connection {
  /// Bound IP address.
  ///
  /// # Errors
  /// Returns socket errors or rejects non-IP addresses.
  pub fn local_address(&self) -> io::Result<SocketAddr> {
    self
      .socket
      .local_addr()?
      .as_socket()
      .ok_or_else(|| io::ErrorKind::Unsupported.into())
  }

  /// Connected peer IP address.
  ///
  /// # Errors
  /// Returns socket errors or rejects non-IP addresses.
  pub fn remote_address(&self) -> io::Result<SocketAddr> {
    self
      .socket
      .peer_addr()?
      .as_socket()
      .ok_or_else(|| io::ErrorKind::Unsupported.into())
  }

  /// Shut down a socket direction without invalidating outstanding buffer ownership.
  ///
  /// # Errors
  /// Returns the native shutdown error.
  pub fn shutdown(&self, direction: Shutdown) -> io::Result<()> {
    self.socket.shutdown(direction)
  }

  /// Set a supported socket option (1 nodelay, 2 keepalive, 3 receive bytes, 4 send bytes, 5 reuse).
  ///
  /// # Errors
  /// Rejects unsupported options/values and returns native option failures.
  pub fn set_option(&self, option: u32, value: i32) -> io::Result<()> {
    match option {
      1 if (0..=1).contains(&value) => self.socket.set_tcp_nodelay(value != 0),
      2 if (0..=1).contains(&value) => self.socket.set_keepalive(value != 0),
      3 if value > 0 => self.socket.set_recv_buffer_size(value as usize),
      4 if value > 0 => self.socket.set_send_buffer_size(value as usize),
      5 if (0..=1).contains(&value) => self.socket.set_reuse_address(value != 0),
      _ => Err(io::ErrorKind::Unsupported.into()),
    }
  }
}

/// Native operation result, carrying the original storage after kernel access ends.
#[derive(Debug)]
pub enum Event {
  /// Connection establishment finished.
  Connected { id: u64, result: io::Result<()> },
  /// Accepted socket ownership can be moved to another driver thread before attachment.
  Accepted { id: u64, result: io::Result<Socket> },
  /// Received initialized bytes; zero indicates EOF.
  Received {
    id: u64,
    result: io::Result<usize>,
    buffer: Buffer,
  },
  /// A persistent receive result; the logical operation survives terminal kernel completions.
  PersistentReceived {
    id: u64,
    result: io::Result<usize>,
    buffer: Option<Buffer>,
    credit: Option<ReceiveCredit>,
    terminal: bool,
  },
  /// All receive, send, and cancellation obligations for a logical receive have retired.
  PersistentRetired { id: u64 },
  /// Completed a send, possibly partially.
  Sent {
    id: u64,
    result: io::Result<usize>,
    buffer: FrozenBuffer,
  },
  /// Completed a vectored send, possibly partially. The vector returns for reuse; a short
  /// write is resumed by `VectoredSend::advance` before resubmitting the same allocation.
  SentVectored {
    id: u64,
    result: io::Result<usize>,
    buffers: Vec<FrozenBuffer>,
  },
}

/// Resumption of a vectored send whose completion moved fewer bytes than were queued.
pub trait VectoredSend {
  /// Drop fully sent views and re-slice the partially sent one, leaving exactly the bytes the
  /// peer has not accepted. Returns true once nothing remains; otherwise the same vector is
  /// ready to resubmit without reallocating.
  ///
  /// # Errors
  /// Rejects a count beyond the queued total, and rejects zero progress over a non-empty
  /// remainder, which would otherwise resubmit an identical iovec forever.
  fn advance(&mut self, sent: usize) -> io::Result<bool>;
}

impl VectoredSend for Vec<FrozenBuffer> {
  fn advance(&mut self, sent: usize) -> io::Result<bool> {
    let total: usize = self.iter().map(|view| view.as_ref().len()).sum();
    if sent > total {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    if sent == 0 && total != 0 {
      return Err(io::ErrorKind::WriteZero.into());
    }
    if sent == total {
      self.clear();
      return Ok(true);
    }
    let mut offset = sent;
    let mut index = 0;
    while offset >= self[index].as_ref().len() {
      offset -= self[index].as_ref().len();
      index += 1;
    }
    let length = self[index].as_ref().len();
    self[index] = self[index].slice(offset..length)?;
    self.drain(..index);
    Ok(false)
  }
}

type Receive = op::Recv<Buffer, SharedFd<Socket>>;
type Send = op::Send<FrozenBuffer, SharedFd<Socket>>;
type SendVectored = op::SendVectored<Vec<FrozenBuffer>, SharedFd<Socket>>;
type Connect = op::Connect<SharedFd<Socket>>;
#[cfg(unix)]
type ConnectPoll = op::PollOnce<SharedFd<Socket>>;
#[cfg(unix)]
type Accept = op::Accept<SharedFd<Socket>>;
#[cfg(windows)]
type Accept = op::Accept<SharedFd<Socket>, Socket>;
type ReadyQueue = Arc<Mutex<VecDeque<u64>>>;

enum Pending {
  Connect(Key<Connect>, SharedFd<Socket>),
  /// A connect io_uring completed with EINPROGRESS, waiting for writability to read SO_ERROR.
  #[cfg(unix)]
  ConnectPoll(Key<ConnectPoll>, SharedFd<Socket>),
  Accept(Key<Accept>),
  Receive(Key<Receive>),
  Send(Key<Send>),
  SendVectored(Key<SendVectored>),
}

struct Operation {
  pending: Pending,
  cancel: Cancel,
  lane: Rc<Cell<bool>>,
}

struct Notification {
  id: u64,
  ready: ReadyQueue,
}

impl Wake for Notification {
  fn wake(self: Arc<Self>) {
    self.wake_by_ref();
  }

  fn wake_by_ref(self: &Arc<Self>) {
    self.ready.lock().unwrap_or_else(|e| e.into_inner()).push_back(self.id);
  }
}

/// Thread-affine driver with bounded admission and completion batches.
pub struct Driver {
  proactor: ManuallyDrop<Proactor>,
  #[cfg(target_os = "linux")]
  persistent: Option<uring::Persistent>,
  owner: Rc<()>,
  pending: IntMap<u64, Operation>,
  ready: ReadyQueue,
  ready_batch: Vec<u64>,
  completed: VecDeque<Event>,
  completed_transient: usize,
  next_id: u64,
  limit: usize,
  closing: bool,
  failed: bool,
  diagnostics: bool,
  fallback: Option<io::Error>,
}

/// io_uring facility the kernel refused; keeps its error reachable through [`raw_os_error`].
#[derive(Debug)]
struct RingSetup {
  requirement: &'static str,
  error: io::Error,
}

impl RingSetup {
  fn wrap(requirement: &'static str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), Self { requirement, error })
  }

  fn is(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Self>())
  }
}

impl std::fmt::Display for RingSetup {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(formatter, "{}: {}", self.requirement, self.error)
  }
}

impl std::error::Error for RingSetup {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    Some(&self.error)
  }
}

/// OS error code of `error` or of the first I/O error in its source chain.
pub fn raw_os_error(error: &io::Error) -> Option<i32> {
  let mut current = error.get_ref().map(|inner| inner as &(dyn std::error::Error + 'static));
  error.raw_os_error().or_else(|| {
    while let Some(inner) = current {
      if let Some(code) = inner.downcast_ref::<io::Error>().and_then(io::Error::raw_os_error) {
        return Some(code);
      }
      current = inner.source();
    }
    None
  })
}

impl Driver {
  /// Create a driver without starting a separate executor. On Linux, [`Backend::Auto`] prefers
  /// io_uring and falls back to polling on any refused io_uring setup (seccomp `EPERM`, `EINVAL` from
  /// kernels before 6.1 without `SINGLE_ISSUER`/`DEFER_TASKRUN`, `ENOSYS`, `ENOMEM` under
  /// memory-cgroup pressure), since every guest run hosts its context on a driver. Invalid bounds
  /// never fall back. [`Driver::fallback`] keeps the refused setup's error.
  ///
  /// # Errors
  /// Rejects invalid bounds, unsupported explicit backends, and native driver initialization failures.
  pub fn new(backend: Backend, limit: usize) -> io::Result<Self> {
    match Self::create(backend, limit) {
      Err(error)
        if cfg!(target_os = "linux")
          && backend == Backend::Auto
          && (RingSetup::is(&error) || error.kind() != io::ErrorKind::InvalidInput) =>
      {
        let mut driver = Self::create(Backend::Polling, limit)?;
        driver.fallback = Some(error);
        Ok(driver)
      }
      created => created,
    }
  }

  /// Why [`Backend::Auto`] runs polling instead of io_uring; `None` when the requested backend runs.
  pub fn fallback(&self) -> Option<&io::Error> {
    self.fallback.as_ref()
  }

  fn create(backend: Backend, limit: usize) -> io::Result<Self> {
    if limit == 0 || limit > 65536 {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut builder = Proactor::builder();
    // Bound submission storage separately from the persistent socket slab and transient admission.
    // Both cancellation paths retry after progress if mixed owner submissions fill the SQ.
    let capacity = if cfg!(target_os = "linux") && backend != Backend::Polling {
      limit
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or(io::ErrorKind::InvalidInput)?
    } else {
      limit
    };
    builder.capacity(u32::try_from(capacity).map_err(|_| io::ErrorKind::InvalidInput)?);
    let requested = match backend {
      Backend::Auto if cfg!(target_os = "linux") => Some(DriverType::IoUring),
      Backend::Auto => None,
      Backend::Polling => Some(DriverType::Poll),
      Backend::IoUring => Some(DriverType::IoUring),
      Backend::Iocp => Some(DriverType::IOCP),
    };
    if let Some(kind) = requested {
      if (kind == DriverType::IoUring && !cfg!(target_os = "linux"))
        || (kind == DriverType::IOCP && !cfg!(windows))
        || (kind == DriverType::Poll && cfg!(windows))
      {
        return Err(io::ErrorKind::Unsupported.into());
      }
      builder.driver_type(kind);
      if kind == DriverType::IoUring {
        builder
          .single_issuer(true)
          .defer_taskrun(true)
          .taskrun_flag(true)
          .coop_taskrun(false);
      }
    }
    if requested == Some(DriverType::IoUring) {
      // Two selected buffers, a receive terminal, one send, and two cancel results per socket.
      let completions = limit
        .checked_mul(6)
        .and_then(|n| n.checked_add(8))
        .ok_or(io::ErrorKind::InvalidInput)?;
      builder.cqsize(u32::try_from(completions.next_power_of_two()).map_err(|_| io::ErrorKind::InvalidInput)?);
    }
    let mut retries = 0;
    #[allow(unused_mut)]
    let mut proactor = loop {
      match builder.build() {
        Ok(proactor) => break proactor,
        Err(error)
          if cfg!(target_os = "linux")
            && requested == Some(DriverType::IoUring)
            && error.kind() == io::ErrorKind::OutOfMemory
            && retries < 10 =>
        {
          // Closed io_uring rings release locked memory asynchronously. Bound startup retries.
          retries += 1;
          std::thread::sleep(Duration::from_millis(10));
        }
        Err(error) if requested == Some(DriverType::IoUring) => {
          return Err(RingSetup::wrap(
            "io_uring setup requires SINGLE_ISSUER, DEFER_TASKRUN, and TASKRUN_FLAG",
            error,
          ));
        }
        Err(error) => return Err(error),
      }
    };
    // CompIO ignores explicit selection when its fusion driver is absent.
    if requested.is_some_and(|kind| kind != proactor.driver_type()) {
      return Err(io::ErrorKind::Unsupported.into());
    }
    #[cfg(target_os = "linux")]
    let (proactor, persistent) = if proactor.driver_type() == DriverType::IoUring {
      let (proactor, persistent) = uring::Persistent::new(proactor, limit).map_err(|error| {
        RingSetup::wrap(
          "io_uring owner setup requires fixed files, buffer rings, and multishot receive",
          error,
        )
      })?;
      (proactor, Some(persistent))
    } else {
      (proactor, None)
    };
    Ok(Self {
      proactor: ManuallyDrop::new(proactor),
      #[cfg(target_os = "linux")]
      persistent,
      owner: Rc::new(()),
      pending: IntMap::with_capacity_and_hasher(limit.min(4096), Default::default()),
      ready: Arc::new(Mutex::new(VecDeque::new())),
      ready_batch: Vec::new(),
      completed: VecDeque::new(),
      completed_transient: 0,
      next_id: 1,
      limit,
      closing: false,
      failed: false,
      diagnostics: std::env::var("ELIDE_TRANSPORT_DIAGNOSTICS").as_deref() == Ok("1"),
      fallback: None,
    })
  }

  fn fail(&mut self, error: io::Error) -> io::Error {
    self.failed = true;
    self.closing = true;
    error
  }

  fn check_failed(&self) -> io::Result<()> {
    if self.failed {
      Err(io::Error::new(
        io::ErrorKind::BrokenPipe,
        "driver failed; unretired kernel resources remain retained",
      ))
    } else {
      Ok(())
    }
  }

  fn emit_diagnostics(&self) {
    #[cfg(target_os = "linux")]
    let owner = unsafe { libc::syscall(libc::SYS_gettid) };
    #[cfg(not(target_os = "linux"))]
    let owner = format!("{:?}", std::thread::current().id());
    eprintln!(
      "elide transport diagnostics owner={owner} mode={} stats={:?}",
      self.backend_description(),
      self.persistent_stats()
    );
  }

  /// Actual selected backend.
  pub fn backend(&self) -> Backend {
    match self.proactor.driver_type() {
      DriverType::Poll => Backend::Polling,
      DriverType::IoUring => Backend::IoUring,
      DriverType::IOCP => Backend::Iocp,
    }
  }

  /// Selected backend and required ring setup, for initialization diagnostics.
  pub fn backend_description(&self) -> &'static str {
    match self.backend() {
      Backend::IoUring => "io_uring (SINGLE_ISSUER | DEFER_TASKRUN | TASKRUN_FLAG)",
      Backend::Polling => "polling",
      Backend::Iocp => "IOCP",
      Backend::Auto => unreachable!("backend selection is resolved during initialization"),
    }
  }

  /// Thread-safe wakeup handle; does not invoke callbacks.
  pub fn waker(&self) -> Waker {
    self.proactor.waker()
  }

  /// Accepted operations whose completion has not yet been delivered.
  pub fn outstanding(&self) -> usize {
    self.pending.len() + self.completed.len() + self.persistent_outstanding()
  }

  /// Owner-local counters; absent for explicitly selected non-io_uring backends.
  pub fn persistent_stats(&self) -> Option<PersistentStats> {
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &self.persistent {
      return Some(persistent.stats());
    }
    None
  }

  fn complete_event(&mut self, event: Event) {
    self.completed_transient += usize::from(Self::is_transient(&event));
    self.completed.push_back(event);
  }

  fn is_transient(event: &Event) -> bool {
    !matches!(
      event,
      Event::PersistentReceived { .. } | Event::PersistentRetired { .. }
    )
  }

  fn transient_outstanding(&self) -> usize {
    let transient = self.pending.len() + self.completed_transient;
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &self.persistent {
      return transient + persistent.transient_outstanding();
    }
    transient
  }

  fn persistent_outstanding(&self) -> usize {
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &self.persistent {
      return persistent.outstanding();
    }
    0
  }

  fn persistent_empty(&self) -> bool {
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &self.persistent {
      return persistent.is_empty();
    }
    true
  }

  /// Enable connection-scoped persistent receives on an idle plaintext HTTP socket.
  /// The returned id remains live until `PersistentRetired`, including across exhaustion and pauses.
  ///
  /// # Errors
  /// Rejects unsupported backends, non-idle sockets, invalid bounds, and native registration failures.
  pub fn enable_persistent_receive(
    &mut self,
    connection: &Connection,
    budget: Budget,
    capacity: usize,
    window: usize,
  ) -> io::Result<u64> {
    #[cfg(target_os = "linux")]
    {
      if self.persistent.is_none() {
        return Err(io::ErrorKind::Unsupported.into());
      }
      if connection.write.get() || connection.persistent.get().is_some() {
        return Err(io::ErrorKind::WouldBlock.into());
      }
      let id = self.admit(connection, &connection.read)?;
      self
        .persistent
        .as_mut()
        .unwrap()
        .enable(&mut self.proactor, connection, id, budget, capacity, window)?;
      Ok(id)
    }
    #[cfg(not(target_os = "linux"))]
    {
      let _ = (connection, budget, capacity, window);
      Err(io::ErrorKind::Unsupported.into())
    }
  }

  /// Stop publication and request receive cancellation, retaining the logical registration.
  ///
  /// # Errors
  /// Rejects missing, closed, or unsupported persistent registrations.
  pub fn pause_persistent_receive(&mut self, id: u64) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &mut self.persistent {
      return persistent.pause(id, true);
    }
    let _ = id;
    Err(io::ErrorKind::Unsupported.into())
  }

  /// Resume admission; rearm waits for receive termination and available buffer credit.
  ///
  /// # Errors
  /// Rejects missing, closed, or unsupported persistent registrations.
  pub fn resume_persistent_receive(&mut self, id: u64) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &mut self.persistent {
      return persistent.pause(id, false);
    }
    let _ = id;
    Err(io::ErrorKind::Unsupported.into())
  }

  /// Attach an owned socket. Pending operations retain it independently of this handle.
  ///
  /// # Errors
  /// Returns socket configuration or driver registration failures.
  pub fn attach(&mut self, socket: Socket) -> io::Result<Connection> {
    if self.closing {
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    if Rc::strong_count(&self.owner) > self.limit {
      return Err(io::ErrorKind::WouldBlock.into());
    }
    socket.set_nonblocking(true)?;
    #[cfg(target_vendor = "apple")]
    socket.set_nosigpipe(true)?;
    self.proactor.attach(socket.as_raw_fd())?;
    Ok(Connection {
      socket: SharedFd::new(socket),
      owner: self.owner.clone(),
      read: Rc::new(Cell::new(false)),
      write: Rc::new(Cell::new(false)),
      #[cfg(target_os = "linux")]
      persistent: Rc::new(Cell::new(None)),
    })
  }

  /// Bind a TCP listener without starting an accept operation.
  ///
  /// # Errors
  /// Returns socket, bind, listen, or attachment failures.
  pub fn listen(&mut self, address: SocketAddr, backlog: i32) -> io::Result<Connection> {
    self.listen_configured(address, backlog, true)
  }

  /// Bind a listener with address-reuse policy applied before bind.
  ///
  /// # Errors
  /// Returns invalid backlog, socket, bind, listen, or attachment failures.
  pub fn listen_configured(&mut self, address: SocketAddr, backlog: i32, reuse: bool) -> io::Result<Connection> {
    if backlog < 0 {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
    // Windows rebinds TIME_WAIT ports by default; its SO_REUSEADDR instead binds ports in active use.
    #[cfg(not(windows))]
    socket.set_reuse_address(reuse)?;
    #[cfg(windows)]
    let _ = reuse;
    socket.bind(&address.into())?;
    socket.listen(backlog)?;
    self.attach(socket)
  }

  /// Bind an independent kernel-distributed listener on this driver's owning thread.
  ///
  /// # Errors
  /// Returns unsupported on platforms without implemented TCP reuse-port distribution, or a
  /// socket, bind, listen, or attachment failure. Never substitutes a shared acceptor.
  pub fn listen_sharded(&mut self, address: SocketAddr, backlog: i32) -> io::Result<Connection> {
    #[cfg(target_os = "linux")]
    {
      if backlog < 0 {
        return Err(io::ErrorKind::InvalidInput.into());
      }
      let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
      socket.set_reuse_address(true)?;
      socket.set_reuse_port(true)?;
      // SO_INCOMING_CPU would bias distribution toward NIC receive CPUs instead of all owners.
      socket.bind(&address.into())?;
      socket.listen(backlog)?;
      self.attach(socket)
    }
    #[cfg(not(target_os = "linux"))]
    {
      let _ = (address, backlog);
      Err(io::ErrorKind::Unsupported.into())
    }
  }

  /// Start a TCP connection; the result is delivered as a Connected event.
  ///
  /// # Errors
  /// Returns setup or admission failures before submission.
  pub fn connect(&mut self, address: SocketAddr) -> io::Result<(Connection, u64)> {
    self.connect_endpoint(address.into())
  }

  pub(crate) fn connect_endpoint(&mut self, address: SockAddr) -> io::Result<(Connection, u64)> {
    let socket = Socket::new(
      address.domain(),
      Type::STREAM,
      if address.as_socket().is_some() {
        Some(Protocol::TCP)
      } else {
        None
      },
    )?;
    #[cfg(windows)]
    socket.bind(
      &SocketAddr::new(
        if address.as_socket().is_some_and(|ip| ip.is_ipv4()) {
          std::net::Ipv4Addr::UNSPECIFIED.into()
        } else {
          std::net::Ipv6Addr::UNSPECIFIED.into()
        },
        0,
      )
      .into(),
    )?;
    let connection = self.attach(socket)?;
    let id = self.admit(&connection, &connection.write)?;
    match self.proactor.push(op::Connect::new(connection.socket.clone(), address)) {
      PushEntry::Pending(key) => {
        let cancel = self.proactor.register_cancel(&key);
        let waker = Waker::from(Arc::new(Notification {
          id,
          ready: self.ready.clone(),
        }));
        self.proactor.update_waker(&key, &waker);
        connection.write.set(true);
        self.pending.insert(
          id,
          Operation {
            pending: Pending::Connect(key, connection.socket.clone()),
            cancel,
            lane: connection.write.clone(),
          },
        );
      }
      PushEntry::Ready(result) => match connect_done(&mut self.proactor, &self.ready, id, result, &connection.socket) {
        ConnectOutcome::Done(event) => self.complete_event(event),
        ConnectOutcome::Waiting(pending, cancel) => {
          connection.write.set(true);
          self.pending.insert(
            id,
            Operation {
              pending,
              cancel,
              lane: connection.write.clone(),
            },
          );
        }
      },
    }
    Ok((connection, id))
  }

  /// Start one accept; no new accept is submitted until requested by the consumer.
  ///
  /// # Errors
  /// Returns admission or socket preparation failures.
  pub fn accept(&mut self, listener: &Connection) -> io::Result<u64> {
    let id = self.admit(listener, &listener.read)?;
    #[cfg(unix)]
    let operation = op::Accept::new(listener.socket.clone());
    #[cfg(windows)]
    let operation = op::Accept::new(
      listener.socket.clone(),
      Socket::new(
        Domain::for_address(listener.local_address()?),
        Type::STREAM,
        Some(Protocol::TCP),
      )?,
    );
    match self.proactor.push(operation) {
      PushEntry::Pending(key) => {
        let cancel = self.proactor.register_cancel(&key);
        let waker = Waker::from(Arc::new(Notification {
          id,
          ready: self.ready.clone(),
        }));
        self.proactor.update_waker(&key, &waker);
        listener.read.set(true);
        self.pending.insert(
          id,
          Operation {
            pending: Pending::Accept(key),
            cancel,
            lane: listener.read.clone(),
          },
        );
      }
      PushEntry::Ready(result) => self.complete_event(accepted(id, result)),
    }
    Ok(id)
  }

  fn admit(&mut self, connection: &Connection, lane: &Cell<bool>) -> io::Result<u64> {
    if self.closing {
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    if !Rc::ptr_eq(&self.owner, &connection.owner) {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    if lane.get() || self.transient_outstanding() >= self.limit {
      return Err(io::ErrorKind::WouldBlock.into());
    }
    let id = self.next_id;
    self.next_id = id.checked_add(1).ok_or(io::ErrorKind::OutOfMemory)?;
    Ok(id)
  }

  /// Submit one receive, transferring exclusive buffer ownership until completion.
  ///
  /// # Errors
  /// Returns the original buffer with the error if admission fails.
  pub fn receive(&mut self, connection: &Connection, buffer: Buffer) -> Result<u64, (io::Error, Buffer)> {
    let id = match self.admit(connection, &connection.read) {
      Ok(id) => id,
      Err(error) => return Err((error, buffer)),
    };
    let operation = op::Recv::new(connection.socket.clone(), buffer, op::RecvFlags::empty());
    match self.proactor.push(operation) {
      PushEntry::Pending(key) => {
        let cancel = self.proactor.register_cancel(&key);
        let waker = Waker::from(Arc::new(Notification {
          id,
          ready: self.ready.clone(),
        }));
        self.proactor.update_waker(&key, &waker);
        connection.read.set(true);
        self.pending.insert(
          id,
          Operation {
            pending: Pending::Receive(key),
            cancel,
            lane: connection.read.clone(),
          },
        );
      }
      PushEntry::Ready(result) => self.complete_event(received(id, result)),
    }
    Ok(id)
  }

  /// Submit a send; its immutable lease prevents mutation until native completion.
  ///
  /// # Errors
  /// Returns the original view with the error if admission fails.
  pub fn send(&mut self, connection: &Connection, buffer: FrozenBuffer) -> Result<u64, (io::Error, FrozenBuffer)> {
    let id = match self.admit(connection, &connection.write) {
      Ok(id) => id,
      Err(error) => return Err((error, buffer)),
    };
    #[cfg(target_os = "linux")]
    if let Some(index) = connection.persistent.get() {
      return self
        .persistent
        .as_mut()
        .expect("persistent backend")
        .send(&mut self.proactor, index, id, buffer)
        .map(|()| id);
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    let flags = op::SendFlags::NOSIGNAL;
    #[cfg(any(windows, target_vendor = "apple"))]
    let flags = op::SendFlags::empty();
    let operation = op::Send::new(connection.socket.clone(), buffer, flags);
    match self.proactor.push(operation) {
      PushEntry::Pending(key) => {
        let cancel = self.proactor.register_cancel(&key);
        let waker = Waker::from(Arc::new(Notification {
          id,
          ready: self.ready.clone(),
        }));
        self.proactor.update_waker(&key, &waker);
        connection.write.set(true);
        self.pending.insert(
          id,
          Operation {
            pending: Pending::Send(key),
            cancel,
            lane: connection.write.clone(),
          },
        );
      }
      PushEntry::Ready(BufResult(result, operation)) => {
        self.complete_event(Event::Sent {
          id,
          result,
          buffer: operation.into_inner(),
        });
      }
    }
    Ok(id)
  }

  /// Submit one vectored send; every view's lease holds until native completion, and the
  /// vector itself returns with the completion so a caller can resubmit or recycle it.
  ///
  /// # Errors
  /// Returns the original views with the error if admission fails.
  pub fn send_vectored(
    &mut self,
    connection: &Connection,
    buffers: Vec<FrozenBuffer>,
  ) -> Result<u64, (io::Error, Vec<FrozenBuffer>)> {
    let id = match self.admit(connection, &connection.write) {
      Ok(id) => id,
      Err(error) => return Err((error, buffers)),
    };
    #[cfg(target_os = "linux")]
    if let Some(index) = connection.persistent.get() {
      return self
        .persistent
        .as_mut()
        .expect("persistent backend")
        .send_vectored(&mut self.proactor, index, id, buffers)
        .map(|()| id);
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    let flags = op::SendFlags::NOSIGNAL;
    #[cfg(any(windows, target_vendor = "apple"))]
    let flags = op::SendFlags::empty();
    let operation = op::SendVectored::new(connection.socket.clone(), buffers, flags);
    match self.proactor.push(operation) {
      PushEntry::Pending(key) => {
        let cancel = self.proactor.register_cancel(&key);
        let waker = Waker::from(Arc::new(Notification {
          id,
          ready: self.ready.clone(),
        }));
        self.proactor.update_waker(&key, &waker);
        connection.write.set(true);
        self.pending.insert(
          id,
          Operation {
            pending: Pending::SendVectored(key),
            cancel,
            lane: connection.write.clone(),
          },
        );
      }
      PushEntry::Ready(BufResult(result, operation)) => {
        self.complete_event(Event::SentVectored {
          id,
          result,
          buffers: operation.into_inner(),
        });
      }
    }
    Ok(id)
  }

  /// Request cancellation. The completion still owns cleanup and buffer return.
  pub fn cancel(&mut self, id: u64) -> bool {
    #[cfg(target_os = "linux")]
    if self.persistent.as_mut().is_some_and(|persistent| persistent.cancel(id)) {
      return true;
    }
    self
      .pending
      .get(&id)
      .is_some_and(|operation| self.proactor.cancel_token(operation.cancel.clone()))
  }

  /// Stop admission and drain cancelled operations within the supplied timeout.
  /// Returns false while native obligations remain; the caller must keep polling or retry shutdown.
  ///
  /// # Errors
  /// Returns native polling errors without releasing outstanding kernel-owned storage.
  pub fn try_shutdown(&mut self, timeout: Duration) -> io::Result<bool> {
    self.check_failed()?;
    if !self.closing {
      self.closing = true;
      #[cfg(target_os = "linux")]
      if let Some(persistent) = &mut self.persistent {
        persistent.close_all();
      }
      for operation in self.pending.values() {
        self.proactor.cancel_token(operation.cancel.clone());
      }
    }
    self.completed.clear();
    self.completed_transient = 0;
    let started = Instant::now();
    while !self.pending.is_empty() || !self.persistent_empty() {
      let remaining = timeout.saturating_sub(started.elapsed());
      drop(self.poll_batch(remaining.min(Duration::from_millis(10)), self.limit)?);
      self.completed.clear();
      self.completed_transient = 0;
      if started.elapsed() >= timeout {
        break;
      }
    }
    Ok(self.pending.is_empty() && self.persistent_empty())
  }

  /// Poll within a timeout and return at most `maximum` completions.
  /// `Duration::MAX` waits indefinitely.
  ///
  /// # Errors
  /// Rejects invalid batch bounds and returns native polling failures.
  pub fn poll(&mut self, timeout: Duration, maximum: usize) -> io::Result<Vec<Event>> {
    Ok(self.poll_batch(timeout, maximum)?.collect())
  }

  #[cfg(test)]
  pub(crate) fn defer_completed(&mut self, events: Vec<Event>) {
    for event in events {
      self.complete_event(event);
    }
  }

  /// Poll into retained completion storage without allocating an output vector.
  /// Dropping the batch discards its remaining events, preserving later queued completions.
  /// `Duration::MAX` waits indefinitely.
  ///
  /// # Errors
  /// Rejects invalid batch bounds and returns native polling failures.
  pub fn poll_batch(
    &mut self,
    timeout: Duration,
    maximum: usize,
  ) -> io::Result<std::collections::vec_deque::Drain<'_, Event>> {
    self.check_failed()?;
    if maximum == 0 || maximum > self.limit {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let previously_completed = self.completed.len();
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &mut self.persistent
      && let Err(error) = persistent.prepare(&mut self.proactor, &mut self.completed)
    {
      return Err(self.fail(error));
    }
    let ready = !self.ready.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
    #[cfg(target_os = "linux")]
    let ready = ready
      || self
        .persistent
        .as_ref()
        .is_some_and(|persistent| persistent.has_ready());
    let timeout = if ready || !self.completed.is_empty() {
      Duration::ZERO
    } else {
      timeout
    };
    if let Err(error) = self.proactor.poll((timeout != Duration::MAX).then_some(timeout))
      && !matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::Interrupted)
    {
      return Err(self.fail(error));
    }
    #[cfg(target_os = "linux")]
    if let Some(persistent) = &mut self.persistent
      && let Err(error) = persistent.drain(&mut self.proactor, &mut self.completed)
    {
      return Err(self.fail(error));
    }
    {
      let mut ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
      let count = maximum.min(ready.len());
      self.ready_batch.extend(ready.drain(..count));
    }
    for id in self.ready_batch.drain(..) {
      let Some(operation) = self.pending.remove(&id) else {
        continue;
      };
      let mut replaced_cancel = None;
      let pending = match operation.pending {
        Pending::Connect(key, socket) => match self.proactor.pop(key) {
          PushEntry::Ready(result) => match connect_done(&mut self.proactor, &self.ready, id, result, &socket) {
            ConnectOutcome::Done(event) => {
              self.completed.push_back(event);
              None
            }
            ConnectOutcome::Waiting(pending, cancel) => {
              replaced_cancel = Some(cancel);
              Some(pending)
            }
          },
          PushEntry::Pending(key) => Some(Pending::Connect(key, socket)),
        },
        #[cfg(unix)]
        Pending::ConnectPoll(key, socket) => match self.proactor.pop(key) {
          PushEntry::Ready(BufResult(result, _)) => {
            self.completed.push_back(polled_connect(id, result, &socket));
            None
          }
          PushEntry::Pending(key) => Some(Pending::ConnectPoll(key, socket)),
        },
        Pending::Accept(key) => match self.proactor.pop(key) {
          PushEntry::Ready(result) => {
            self.completed.push_back(accepted(id, result));
            None
          }
          PushEntry::Pending(key) => Some(Pending::Accept(key)),
        },
        Pending::Receive(key) => match self.proactor.pop(key) {
          PushEntry::Ready(result) => {
            self.completed.push_back(received(id, result));
            None
          }
          PushEntry::Pending(key) => Some(Pending::Receive(key)),
        },
        Pending::Send(key) => match self.proactor.pop(key) {
          PushEntry::Ready(BufResult(result, op)) => {
            self.completed.push_back(Event::Sent {
              id,
              result,
              buffer: op.into_inner(),
            });
            None
          }
          PushEntry::Pending(key) => Some(Pending::Send(key)),
        },
        Pending::SendVectored(key) => match self.proactor.pop(key) {
          PushEntry::Ready(BufResult(result, op)) => {
            self.completed.push_back(Event::SentVectored {
              id,
              result,
              buffers: op.into_inner(),
            });
            None
          }
          PushEntry::Pending(key) => Some(Pending::SendVectored(key)),
        },
      };
      if let Some(pending) = pending {
        self.pending.insert(
          id,
          Operation {
            pending,
            cancel: replaced_cancel.unwrap_or(operation.cancel),
            lane: operation.lane,
          },
        );
      } else {
        operation.lane.set(false);
      }
    }
    // Only inspect this poll's bounded append batch; persistent events have separate credits.
    self.completed_transient += self
      .completed
      .range(previously_completed..)
      .filter(|event| Self::is_transient(event))
      .count();
    let count = maximum.min(self.completed.len());
    self.completed_transient -= self
      .completed
      .range(..count)
      .filter(|event| Self::is_transient(event))
      .count();
    Ok(self.completed.drain(..count))
  }
}

impl Drop for Driver {
  fn drop(&mut self) {
    // Normal callers use try_shutdown. Dropping live I/O must drain IOCP before closing its port.
    loop {
      match self.try_shutdown(Duration::from_millis(10)) {
        Ok(true) => {
          if self.diagnostics {
            self.emit_diagnostics();
          }
          unsafe { ManuallyDrop::drop(&mut self.proactor) };
          break;
        }
        Ok(false) => continue,
        Err(_) => {
          // An unrecoverable poll failure cannot authorize freeing kernel-owned memory.
          std::mem::forget(std::mem::take(&mut self.pending));
          #[cfg(target_os = "linux")]
          std::mem::forget(self.persistent.take());
          break;
        }
      }
    }
  }
}

fn connected(id: u64, BufResult(result, operation): BufResult<usize, Connect>) -> Event {
  #[cfg(windows)]
  let result = result
    .map_err(completion_error)
    .and_then(|_| operation.update_context());
  #[cfg(not(windows))]
  let result = {
    drop(operation);
    result.map(|_| ())
  };
  Event::Connected { id, result }
}

enum ConnectOutcome {
  Done(Event),
  Waiting(Pending, Cancel),
}

/// The Connected event for a finished connect op. io_uring may complete a connect on a
/// non-blocking socket with EINPROGRESS; that is not a failure, so wait for writability and read
/// SO_ERROR, as the poll backend's connect does.
fn connect_done(
  proactor: &mut Proactor,
  ready: &ReadyQueue,
  id: u64,
  result: BufResult<usize, Connect>,
  socket: &SharedFd<Socket>,
) -> ConnectOutcome {
  #[cfg(unix)]
  if let Err(error) = &result.0
    && matches!(error.raw_os_error(), Some(libc::EINPROGRESS | libc::EALREADY))
  {
    return match proactor.push(op::PollOnce::new(socket.clone(), op::Interest::Writable)) {
      PushEntry::Pending(key) => {
        let cancel = proactor.register_cancel(&key);
        let waker = Waker::from(Arc::new(Notification {
          id,
          ready: ready.clone(),
        }));
        proactor.update_waker(&key, &waker);
        ConnectOutcome::Waiting(Pending::ConnectPoll(key, socket.clone()), cancel)
      }
      PushEntry::Ready(BufResult(polled, _)) => ConnectOutcome::Done(polled_connect(id, polled, socket)),
    };
  }
  #[cfg(not(unix))]
  let _ = (proactor, ready, socket);
  ConnectOutcome::Done(connected(id, result))
}

/// The Connected event once an in-progress connect's socket turned writable.
#[cfg(unix)]
fn polled_connect(id: u64, polled: io::Result<usize>, socket: &SharedFd<Socket>) -> Event {
  let result = polled.and_then(|_| match socket.take_error()? {
    Some(error) => Err(error),
    None => Ok(()),
  });
  Event::Connected { id, result }
}

/// Classify Win32 codes from `RtlNtStatusToDosError` that std leaves uncategorized, keeping the code.
#[cfg(windows)]
fn completion_error(error: io::Error) -> io::Error {
  const ERROR_CONNECTION_REFUSED: i32 = 1225;
  const ERROR_PORT_UNREACHABLE: i32 = 1234;
  const ERROR_CONNECTION_ABORTED: i32 = 1236;
  match error.raw_os_error() {
    Some(ERROR_CONNECTION_REFUSED | ERROR_PORT_UNREACHABLE) => io::Error::new(io::ErrorKind::ConnectionRefused, error),
    Some(ERROR_CONNECTION_ABORTED) => io::Error::new(io::ErrorKind::ConnectionAborted, error),
    _ => error,
  }
}

fn accepted(id: u64, BufResult(result, operation): BufResult<usize, Accept>) -> Event {
  #[cfg(windows)]
  let result = result.and_then(|_| {
    operation.update_context()?;
    operation.into_addr().map(|(socket, _)| socket)
  });
  #[cfg(unix)]
  let result = result.map(|_| operation.into_inner().0);
  Event::Accepted { id, result }
}

fn received(id: u64, BufResult(result, operation): BufResult<usize, Receive>) -> Event {
  let mut buffer = operation.into_inner();
  // A failed receive exposes no bytes, even when reusing previously initialized storage.
  unsafe { buffer.set_len(result.as_ref().copied().unwrap_or(0)) };
  Event::Received { id, result, buffer }
}

#[cfg(all(test, unix))]
mod tests {
  use super::*;
  use crate::buffer::Budget;
  use std::net::{TcpListener, TcpStream};

  fn driver() -> Driver {
    let requested = match std::env::var("ELIDE_TRANSPORT_TEST_BACKEND").as_deref() {
      Ok("io-uring") => Backend::IoUring,
      Ok("kqueue" | "epoll") => Backend::Polling,
      _ => Backend::Auto,
    };
    let driver = Driver::new(requested, 4).unwrap();
    if requested != Backend::Auto {
      assert_eq!(driver.backend(), requested);
    }
    driver
  }

  fn ready(driver: &mut Driver) {
    driver.complete_event(Event::Connected {
      id: u64::MAX,
      result: Ok(()),
    });
  }

  #[test]
  #[cfg(target_os = "linux")]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn sharded_listener_does_not_prefer_the_packet_receive_cpu() {
    let mut driver = driver();
    let listener = driver.listen_sharded("127.0.0.1:0".parse().unwrap(), 128).unwrap();
    assert_eq!(listener.socket.cpu_affinity().unwrap() as i32, -1);
  }

  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn immediate_completions_cannot_starve_pending_cancellation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut driver = driver();
    let polling = driver.backend() == Backend::Polling;
    let connection = driver.attach(client.into()).unwrap();
    let budget = Budget::new(32);
    let id = driver
      .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
      .unwrap();
    assert!(driver.cancel(id));
    ready(&mut driver);
    let mut cancelled = false;
    let first = driver.poll(Duration::ZERO, 4).unwrap();
    for event in first {
      cancelled |= matches!(event, Event::Received { id: found, result: Err(_), .. } if found == id);
    }
    ready(&mut driver);
    let second = driver.poll(Duration::ZERO, 4).unwrap();
    for event in second {
      cancelled |= matches!(event, Event::Received { id: found, result: Err(_), .. } if found == id);
    }
    if polling {
      assert!(cancelled, "refilled immediate completions starved cancellation");
    }
    assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
    assert_eq!(budget.used(), 0);
  }

  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn completed_batches_preserve_next_cross_thread_wakeup() {
    let mut driver = driver();
    ready(&mut driver);
    assert_eq!(driver.poll(Duration::ZERO, 1).unwrap().len(), 1);
    let wake = driver.waker();
    std::thread::spawn(move || wake.wake()).join().unwrap();
    let started = Instant::now();
    assert!(driver.poll(Duration::from_secs(5), 1).unwrap().is_empty());
    assert!(started.elapsed() < Duration::from_secs(2));
  }

  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn fatal_poll_failure_remains_fatal_during_shutdown() {
    let mut driver = driver();
    let error = driver.fail(io::ErrorKind::InvalidData.into());
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(driver.poll(Duration::ZERO, 1).is_err());
    assert!(driver.try_shutdown(Duration::ZERO).is_err());
    assert!(driver.try_shutdown(Duration::from_secs(1)).is_err());
    // This fixture has never submitted I/O; clear the injection so it can release its idle ring.
    assert_eq!(driver.outstanding(), 0);
    driver.failed = false;
  }

  #[cfg(target_os = "linux")]
  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn queued_persistent_receive_events_do_not_starve_responses() {
    let mut driver = driver();
    if driver.backend() != Backend::IoUring {
      return;
    }
    driver.limit = 1;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let connection = driver.attach(client.into()).unwrap();
    let budget = Budget::new(65536);
    let id = driver
      .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
      .unwrap();
    // Inject only owner delivery state: queued receives have their own bounded credit domain.
    driver.defer_completed(
      (0..2)
        .map(|_| Event::PersistentReceived {
          id,
          result: Ok(0),
          buffer: None,
          credit: None,
          terminal: false,
        })
        .collect(),
    );
    let mut output = Buffer::new(1, budget.clone()).unwrap();
    output.write(0, b"x").unwrap();
    driver.send(&connection, output.freeze()).unwrap();
    assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
    assert_eq!(budget.used(), 0);
  }
}
