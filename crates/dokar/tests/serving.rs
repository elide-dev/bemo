mod common;

use dokar::abi::*;
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn frozen(owner: u64, bytes: &[u8]) -> u64 {
  let buffer = elide_transport_buffer_new(owner, bytes.len() as u64);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(buffer, &mut view) }, 0);
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.address.cast(), bytes.len()) };
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(buffer, bytes.len() as u64) }, 0);
  buffer
}

fn listen(driver: u64, owner: u64) -> (u64, u16) {
  let mut address = [0u8; 24];
  address[..4].copy_from_slice(&[127, 0, 0, 1]);
  address[18..20].copy_from_slice(&4u16.to_ne_bytes());
  let endpoint = frozen(owner, &address);
  let signature = frozen(owner, b"same listener configuration");
  let listener = elide_transport_serving_listen(common::workload(), driver, endpoint, signature, 64);
  assert_ne!(listener, 0);
  elide_transport_buffer_release(endpoint);
  elide_transport_buffer_release(signature);
  let output = elide_transport_buffer_new(owner, 24);
  assert_eq!(elide_transport_socket_address(driver, listener, 0, output), 0);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(output, &mut view) }, 0);
  // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
  let bytes = unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), 24) };
  let port = u16::from_ne_bytes(bytes[16..18].try_into().unwrap());
  elide_transport_buffer_release(output);
  (listener, port)
}

fn release(driver: u64, owner: u64, batch: u64) {
  let deadline = Instant::now() + Duration::from_secs(3);
  while elide_transport_driver_release(driver) == BUSY {
    assert!(Instant::now() < deadline);
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    unsafe { elide_transport_driver_poll(driver, 1_000_000, batch, 8) };
  }
  elide_transport_buffer_release(batch);
  assert_eq!(elide_transport_owner_used(owner), 0);
  elide_transport_owner_release(owner);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn split_serving_hands_connections_to_a_dedicated_transport_owner() {
  let application = elide_transport_serving_new(1);
  let driver = elide_transport_serving_driver(common::workload(), application, 0, common::backend() as u32, 32, 2);
  assert_ne!(driver, 0, "split serving endpoint unavailable");
  let owner = elide_transport_owner_new(1024 * 1024);
  let batch = elide_transport_buffer_new(owner, 8 * 40);
  let (_, port) = listen(driver, owner);
  let peer = TcpStream::connect(("127.0.0.1", port)).unwrap();
  assert_eq!(elide_transport_serving_ready(driver), 0);
  let deadline = Instant::now() + Duration::from_secs(3);
  let accepted = loop {
    assert!(Instant::now() < deadline);
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, 10_000_000, batch, 8) };
    assert!(count >= 0);
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
    let events = unsafe { std::slice::from_raw_parts(view.address.cast::<NativeEvent>(), count as usize) };
    if let Some(event) = events.iter().find(|event| event.kind == 2) {
      assert_eq!(event.result, 0);
      break event.value;
    }
  };
  std::thread::spawn(move || {
    let transport = elide_transport_driver_new(common::workload(), common::backend() as u32, 32);
    assert_ne!(transport, 0);
    assert_eq!(elide_transport_socket_adopt(common::workload(), transport, accepted), 0);
    assert_eq!(elide_transport_socket_option(transport, accepted, 1, 1), 0);
    assert_eq!(elide_transport_socket_close(transport, accepted), 0);
    assert_eq!(elide_transport_driver_release(transport), 0);
  })
  .join()
  .unwrap();
  assert_eq!(elide_transport_serving_close(application), 0);
  drop(peer);
  release(driver, owner, batch);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_readiness_gates_accept_and_adoption_stays_on_the_polling_thread() {
  let application = elide_transport_serving_new(1);
  assert_ne!(application, 0);
  let driver = elide_transport_serving_driver(common::workload(), application, 0, common::backend() as u32, 32, 1);
  assert_ne!(driver, 0);
  let owner = elide_transport_owner_new(1024 * 1024);
  let batch = elide_transport_buffer_new(owner, 8 * 40);
  let (listener, port) = listen(driver, owner);
  let peer = TcpStream::connect(("127.0.0.1", port)).unwrap();
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 0);
  assert_eq!(elide_transport_serving_ready(driver), 0);
  let deadline = Instant::now() + Duration::from_secs(3);
  let accepted = loop {
    assert!(Instant::now() < deadline);
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, 10_000_000, batch, 8) };
    assert!(count >= 0);
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
    let events = unsafe { std::slice::from_raw_parts(view.address.cast::<NativeEvent>(), count as usize) };
    if let Some(event) = events.iter().find(|event| event.kind == 2) {
      assert_eq!(event.socket, listener);
      assert_eq!(event.result, 0);
      break event.value;
    }
  };
  // Already attached to this reactor, not published through the cross-thread accept registry.
  assert_eq!(elide_transport_socket_discard(accepted), INVALID);
  assert_eq!(elide_transport_socket_option(driver, accepted, 1, 1), 0);
  std::thread::spawn(move || {
    assert_eq!(elide_transport_driver_backend(driver), INVALID);
    assert_eq!(elide_transport_serving_ready(driver), INVALID);
  })
  .join()
  .unwrap();
  assert_eq!(elide_transport_serving_close(application), 0);
  drop(peer);
  release(driver, owner, batch);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn serving_rejects_duplicate_endpoint_ownership() {
  let application = elide_transport_serving_new(1);
  assert_ne!(application, 0);
  let driver = elide_transport_serving_driver(common::workload(), application, 0, common::backend() as u32, 8, 1);
  assert_ne!(driver, 0);
  assert_eq!(
    elide_transport_serving_driver(common::workload(), application, 0, common::backend() as u32, 8, 1),
    0
  );
  assert_eq!(elide_transport_serving_close(application), 0);
  assert_eq!(elide_transport_driver_release(driver), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn lifecycle_notifications_do_not_require_a_listener() {
  let application = elide_transport_serving_new(1);
  let driver = elide_transport_serving_driver(common::workload(), application, 0, common::backend() as u32, 8, 1);
  assert_ne!(driver, 0);
  let owner = elide_transport_owner_new(1024);
  let batch = elide_transport_buffer_new(owner, 8 * 40);
  assert_eq!(elide_transport_serving_ready(driver), 0);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 1);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
  // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
  let event = unsafe { &*view.address.cast::<NativeEvent>() };
  assert_eq!(event.kind, 12);
  assert_eq!(event.value, 1);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 0);
  assert_eq!(elide_transport_serving_close(application), 0);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 1);
  // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
  let event = unsafe { &*view.address.cast::<NativeEvent>() };
  assert_eq!(event.kind, 12);
  assert_eq!(event.value, 2);
  assert_eq!(elide_transport_serving_ready(driver), INVALID);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 0);
  release(driver, owner, batch);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn shards_share_port_zero_and_native_readiness_and_close() {
  use std::sync::{Arc, Barrier, mpsc};
  let application = elide_transport_serving_new(2);
  assert_ne!(application, 0);
  let barrier = Arc::new(Barrier::new(2));
  let (ports, received) = mpsc::channel();
  let threads: Vec<_> = (0..2)
    .map(|index| {
      let barrier = barrier.clone();
      let ports = ports.clone();
      std::thread::spawn(move || {
        let driver =
          elide_transport_serving_driver(common::workload(), application, index, common::backend() as u32, 32, 1);
        assert_ne!(driver, 0);
        let owner = elide_transport_owner_new(1024 * 1024);
        let batch = elide_transport_buffer_new(owner, 8 * 40);
        let (listener, port) = listen(driver, owner);
        ports.send(port).unwrap();
        barrier.wait();
        if index == 0 {
          assert_eq!(elide_transport_serving_ready(driver), 0);
          // SAFETY: This thread owns the driver and the live batch has capacity for eight events.
          assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 0);
        }
        barrier.wait();
        if index == 1 {
          assert_eq!(elide_transport_serving_ready(driver), 0);
        }
        barrier.wait();
        // SAFETY: This thread owns the driver and the live batch has capacity for eight events.
        assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 1);
        let mut view = BufferView::default();
        // SAFETY: batch is retained and view is writable for the complete BufferView.
        assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
        // SAFETY: poll initialized these events in the retained, NativeEvent-aligned batch allocation.
        assert_eq!(unsafe { &*view.address.cast::<NativeEvent>() }.kind, 12);
        barrier.wait();
        if index == 0 {
          assert_eq!(elide_transport_serving_listener_close(driver, listener), 0);
        }
        barrier.wait();
        // SAFETY: This thread owns the driver and the live batch has capacity for eight events.
        let count = unsafe { elide_transport_driver_poll(driver, 0, batch, 8) };
        assert!(count > 0);
        // SAFETY: poll initialized these events in the retained, NativeEvent-aligned batch allocation.
        let events = unsafe { std::slice::from_raw_parts(view.address.cast::<NativeEvent>(), count as usize) };
        assert_eq!(
          events
            .iter()
            .filter(|event| event.kind == EVENT_LISTENER_CLOSED && event.socket == listener)
            .count(),
          1
        );
        barrier.wait();
        release(driver, owner, batch);
      })
    })
    .collect();
  let first = received.recv_timeout(Duration::from_secs(5)).unwrap();
  assert_ne!(first, 0);
  assert_eq!(first, received.recv_timeout(Duration::from_secs(5)).unwrap());
  for thread in threads {
    thread.join().unwrap();
  }
  assert_eq!(elide_transport_serving_close(application), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn shard_thread_is_pinned_and_restores_its_original_allowed_cpus() {
  fn mask() -> [usize; libc::CPU_SETSIZE as usize / usize::BITS as usize] {
    let mut set = [0usize; libc::CPU_SETSIZE as usize / usize::BITS as usize];
    assert_eq!(
      // SAFETY: set is writable and word-aligned; the kernel receives its exact byte capacity.
      unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&set), set.as_mut_ptr().cast()) },
      0
    );
    set
  }
  let application = elide_transport_serving_new(1);
  std::thread::spawn(move || {
    let before = mask();
    let driver = elide_transport_serving_driver(common::workload(), application, 0, common::backend() as u32, 8, 1);
    assert_ne!(driver, 0);
    let cpu = elide_transport_serving_cpu(driver);
    assert!(cpu >= 0);
    let bit = cpu as usize;
    assert_ne!(
      before[bit / usize::BITS as usize] & (1usize << (bit % usize::BITS as usize)),
      0
    );
    assert_eq!(mask().iter().map(|word| word.count_ones()).sum::<u32>(), 1);
    // SAFETY: sched_getcpu takes no arguments and does not access caller memory.
    assert_eq!(unsafe { libc::sched_getcpu() }, cpu);
    assert_eq!(elide_transport_serving_close(application), 0);
    assert_eq!(elide_transport_driver_release(driver), 0);
    let after = mask();
    assert_eq!(before, after);
  })
  .join()
  .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn secondary_worker_startup_failure_wakes_and_closes_the_first_owner() {
  use std::io::Read;
  use std::sync::mpsc;

  let application = elide_transport_serving_split_new(1);
  assert_ne!(application, 0);
  let workers = elide_transport_serving_workers(application, 0);
  if workers < 2 {
    assert_eq!(elide_transport_serving_close(application), 0);
    eprintln!("secondary worker startup requires two allowed physical cores");
    return;
  }
  let (initialized, first) = mpsc::channel();
  let (start, started) = mpsc::channel();
  let (waiting, poll_started) = mpsc::channel();
  let (finished, stopped) = mpsc::channel();
  let first_owner = std::thread::spawn(move || {
    let driver =
      elide_transport_serving_worker_driver(common::workload(), application, 0, 0, common::backend() as u32, 32);
    assert_ne!(driver, 0);
    let owner = elide_transport_owner_new(1024 * 1024);
    let batch = elide_transport_buffer_new(owner, 8 * 40);
    let (listener, port) = listen(driver, owner);
    assert_eq!(elide_transport_serving_ready(driver), 0);
    initialized.send((driver, port)).unwrap();
    started.recv_timeout(Duration::from_secs(3)).unwrap();
    // The other owners are not ready: the queued peer must not be accepted.
    // SAFETY: This thread owns the driver and the live batch has capacity for eight events.
    assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 0);
    waiting.send(()).unwrap();
    let mut listener_closed = 0;
    let mut application_closed = 0;
    while listener_closed == 0 || application_closed == 0 {
      // SAFETY: This thread owns the driver and the live batch has capacity for eight events.
      let count = unsafe { elide_transport_driver_poll(driver, u64::MAX, batch, 8) };
      assert!(count >= 0);
      let mut view = BufferView::default();
      // SAFETY: batch is retained and view is writable for the complete BufferView.
      assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
      // SAFETY: poll initialized these events in the retained, NativeEvent-aligned batch allocation.
      let events = unsafe { std::slice::from_raw_parts(view.address.cast::<NativeEvent>(), count as usize) };
      for event in events {
        match event.kind {
          EVENT_LISTENER_CLOSED => {
            assert_eq!(event.socket, listener);
            listener_closed += 1;
          }
          EVENT_SERVING_PHASE => {
            assert_eq!(event.value, 2, "failed startup must never publish READY");
            application_closed += 1;
          }
          2 => {
            elide_transport_socket_discard(event.value);
            panic!("startup failure admitted a queued connection");
          }
          _ => panic!("unexpected startup event: {}", event.kind),
        }
      }
    }
    assert_eq!((listener_closed, application_closed), (1, 1));
    assert_eq!(elide_transport_serving_ready(driver), INVALID);
    assert_eq!(elide_transport_socket_option(driver, listener, 1, 1), INVALID);
    // SAFETY: This thread owns the driver and the live batch has capacity for eight events.
    assert_eq!(unsafe { elide_transport_driver_poll(driver, 0, batch, 8) }, 0);
    release(driver, owner, batch);
    finished.send(()).unwrap();
  });
  let (driver, port) = first.recv_timeout(Duration::from_secs(5)).unwrap();
  let mut peer = TcpStream::connect(("127.0.0.1", port)).unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
  start.send(()).unwrap();
  poll_started.recv_timeout(Duration::from_secs(3)).unwrap();
  // A valid secondary assignment with a forbidden limit forces driver creation to fail,
  // rather than failing the worker-index validation before startup coordination is entered.
  assert_eq!(
    elide_transport_serving_worker_driver(common::workload(), application, 0, 1, common::backend() as u32, 0),
    0
  );
  assert_eq!(elide_transport_serving_workers(application, 0), INVALID);
  assert_eq!(
    elide_transport_serving_worker_driver(common::workload(), application, 0, 1, common::backend() as u32, 32),
    0
  );
  let woke = stopped.recv_timeout(Duration::from_secs(3)).is_ok();
  if !woke {
    // Rescue a missing-wake regression so the test fails without leaving a parked owner.
    elide_transport_serving_close(application);
    elide_transport_driver_wake(driver);
  }
  first_owner.join().unwrap();
  assert!(
    woke,
    "secondary startup failure did not wake and retire the first owner"
  );
  assert_eq!(elide_transport_serving_close(application), 0);
  match peer.read(&mut [0]) {
    Ok(0) => {}
    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
    outcome => panic!("failed listener retained its queued peer: {outcome:?}"),
  }
}

#[test]
#[cfg_attr(miri, ignore = "serving placement reads the CPU affinity mask")]
fn serving_entry_points_reject_unknown_handles_and_out_of_range_indices() {
  assert!(elide_transport_serving_available_cores() > 0);
  assert_eq!(elide_transport_serving_new(0), 0);
  assert_eq!(elide_transport_last_error(), -3, "zero contexts is unsupported");
  assert_eq!(elide_transport_serving_workers(0, 0), INVALID);
  assert_eq!(
    elide_transport_serving_worker_driver(common::workload(), 0, 0, 0, 0, 8),
    0
  );
  assert_eq!(elide_transport_serving_driver(common::workload(), 0, 0, 0, 8, 0), 0);
  assert_eq!(
    elide_transport_serving_driver(common::workload(), 0, 0, 0, 8, 4),
    0,
    "unknown placement bits"
  );
  assert_eq!(elide_transport_serving_listen(common::workload(), 0, 0, 0, 16), 0);
  assert_eq!(elide_transport_serving_ready(0), INVALID);
  assert_eq!(elide_transport_serving_cpu(0), -1);
  assert_eq!(elide_transport_serving_listener_close(0, 0), INVALID);
  assert_eq!(elide_transport_serving_context_enter(0, 0), INVALID);
  assert_eq!(elide_transport_last_error(), -3);
  assert_eq!(elide_transport_serving_context_leave(), INVALID, "nothing was entered");
  // SAFETY: Zero denotes no owned helper token and is explicitly accepted by the ABI.
  assert_eq!(unsafe { elide_transport_serving_helper_release(0) }, 0);
  // SAFETY: Zero denotes no owned helper token and is explicitly accepted by the ABI.
  assert_eq!(unsafe { elide_transport_serving_helper_start(0) }, 0);
  assert_eq!(elide_transport_serving_close(0), INVALID);

  let application = elide_transport_serving_new(1);
  assert_ne!(application, 0);
  assert_eq!(elide_transport_serving_workers(application, 0), 1);
  assert_eq!(elide_transport_serving_workers(application, 1), INVALID);
  assert_eq!(elide_transport_serving_context_enter(application, 1), INVALID);
  assert_eq!(
    elide_transport_serving_worker_driver(common::workload(), application, 1, 0, 0, 8),
    0
  );
  assert_eq!(
    elide_transport_serving_worker_driver(common::workload(), application, 0, 1, 0, 8),
    0
  );
  assert_eq!(
    elide_transport_serving_worker_driver(common::workload(), application, 0, u32::MAX, 0, 8),
    0
  );
  assert_eq!(
    elide_transport_serving_driver(common::workload(), application, 1, 0, 8, 0),
    0,
    "no such replica"
  );
  assert_eq!(elide_transport_serving_close(application), 0);
  assert_eq!(elide_transport_serving_close(application), INVALID);
  assert_eq!(elide_transport_serving_workers(application, 0), INVALID);
  assert_eq!(elide_transport_serving_context_enter(application, 0), INVALID);
}
