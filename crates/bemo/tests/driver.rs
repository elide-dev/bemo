mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use bemo::buffer::{Budget, Buffer, FrozenBuffer};
use bemo::driver::{Backend, Driver, Event, VectoredSend, raw_os_error};

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn receive_and_send_return_native_storage() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(32);
  let receive = driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  peer.write_all(b"h").unwrap();
  let Event::Received { id, result, buffer } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(id, receive);
  assert_eq!(result.unwrap(), 1);
  let frozen = buffer.freeze();
  assert_eq!(frozen.as_ref(), b"h");
  let retained = frozen.clone();
  let send = driver.send(&connection, frozen).unwrap();
  let Event::Sent { id, result, buffer } = next(&mut driver) else {
    panic!("send expected")
  };
  assert_eq!(id, send);
  assert_eq!(result.unwrap(), 1);
  drop(buffer);
  let mut reply = [0; 1];
  peer.read_exact(&mut reply).unwrap();
  assert_eq!(&reply, b"h");
  drop(driver);
  assert_eq!(budget.used(), 32);
  drop(retained);
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn cancellation_returns_storage_before_recycling() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(32);
  let id = driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  assert!(driver.cancel(id));
  assert_eq!(budget.used(), 32);
  let event = next(&mut driver);
  assert!(matches!(&event, Event::Received { id: found, result: Err(_), .. } if *found == id));
  assert_eq!(budget.used(), 32);
  drop(event);
  assert_eq!(budget.used(), 0);
  assert!(!driver.cancel(id));
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn cross_thread_wakeup_interrupts_poll() {
  let mut driver = driver(8);
  let wake = driver.waker();
  let thread = std::thread::spawn(move || {
    wake.wake();
  });
  let started = Instant::now();
  driver.poll(Duration::from_secs(5), 8).unwrap();
  assert!(started.elapsed() < Duration::from_secs(2));
  thread.join().unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn auto_reports_why_io_uring_was_refused() {
  // The errno a sandbox returns for io_uring_setup; Docker's default seccomp profile returns EPERM.
  let expected = std::env::var("ELIDE_TRANSPORT_TEST_EXPECT_FALLBACK")
    .ok()
    .map(|errno| errno.parse::<i32>().expect("fallback errno"));
  let driver = Driver::new(Backend::Auto, 8).unwrap();
  match (driver.backend(), driver.fallback()) {
    // Only Linux AUTO tries io_uring first.
    (Backend::Polling, Some(error)) if cfg!(target_os = "linux") => {
      assert!(!error.to_string().is_empty());
      if let Some(errno) = expected {
        assert_eq!(raw_os_error(error), Some(errno), "{error}");
      }
    }
    (backend, None) => assert_eq!(expected, None, "AUTO selected {backend:?} without a refusal"),
    (backend, Some(error)) => panic!("{backend:?} must not report a fallback: {error}"),
  }
  if let Some(errno) = expected {
    let error = Driver::new(Backend::IoUring, 8)
      .err()
      .expect("explicit io_uring never falls back");
    assert_eq!(raw_os_error(&error), Some(errno), "{error}");
  }
  if common::backend() != Backend::Auto {
    assert!(Driver::new(common::backend(), 8).unwrap().fallback().is_none());
  }
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(miri, ignore = "seccomp and io_uring are unavailable under miri")]
fn auto_falls_back_on_every_refused_ring_setup() {
  // EPERM: seccomp. EINVAL: kernels before 6.1 lack SINGLE_ISSUER/DEFER_TASKRUN. ENOSYS: no io_uring.
  for errno in [libc::EPERM, libc::EINVAL, libc::ENOSYS] {
    std::thread::spawn(move || {
      common::refuse_io_uring(errno);
      let driver = Driver::new(Backend::Auto, 8).unwrap_or_else(|error| panic!("errno {errno}: {error}"));
      assert_eq!(driver.backend(), Backend::Polling, "errno {errno}");
      let reason = driver.fallback().expect("fallback reason");
      assert_eq!(raw_os_error(reason), Some(errno), "{reason}");
      let error = Driver::new(Backend::IoUring, 8)
        .err()
        .expect("explicit io_uring never falls back");
      assert_eq!(raw_os_error(&error), Some(errno), "{error}");
      let bounds = Driver::new(Backend::Auto, 0)
        .err()
        .expect("invalid bounds never fall back");
      assert_eq!(bounds.kind(), std::io::ErrorKind::InvalidInput);
    })
    .join()
    .unwrap();
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn refused_connect_reports_connection_refused() {
  let address = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
  let mut driver = driver(8);
  let (_connection, id) = driver.connect(address).unwrap();
  let Event::Connected { id: found, result } = next(&mut driver) else {
    panic!("connect expected")
  };
  assert_eq!(found, id);
  let error = result.expect_err("nothing listens on the released port");
  assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused, "{error}");
}

#[test]
#[cfg(not(target_os = "linux"))]
#[cfg_attr(miri, ignore = "real sockets are unavailable under miri")]
fn persistent_receive_is_unsupported_off_linux() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let unsupported = |result: std::io::Result<()>| {
    assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::Unsupported));
  };
  unsupported(
    driver
      .enable_persistent_receive(&connection, Budget::new(1 << 16), 16384, 1 << 16)
      .map(drop),
  );
  unsupported(driver.pause_persistent_receive(1));
  unsupported(driver.resume_persistent_receive(1));
  assert_eq!(driver.outstanding(), 0);
}

#[test]
fn explicit_unsupported_backend_is_rejected() {
  if !cfg!(target_os = "linux") {
    assert!(Driver::new(Backend::IoUring, 8).is_err());
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn sharded_listeners_require_kernel_load_balancing() {
  let mut first = driver(8);
  let result = first.listen_sharded("127.0.0.1:0".parse().unwrap(), 128);
  #[cfg(target_os = "linux")]
  {
    let listener = result.unwrap();
    let address = listener.local_address().unwrap();
    let mut second = driver(8);
    let other = second.listen_sharded(address, 128).unwrap();
    assert_eq!(other.local_address().unwrap(), address);
    assert_ne!(first.accept(&listener).unwrap(), 0);
    assert_ne!(second.accept(&other).unwrap(), 0);
    let clients: Vec<_> = (0..32)
      .map(|_| TcpStream::connect_timeout(&address, Duration::from_secs(3)).unwrap())
      .collect();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut accepted = [false; 2];
    while !accepted.iter().all(|ready| *ready) && Instant::now() < deadline {
      for (index, driver) in [&mut first, &mut second].into_iter().enumerate() {
        for event in driver.poll(Duration::from_millis(10), 8).unwrap() {
          if let Event::Accepted { result, .. } = event {
            assert!(result.is_ok());
            accepted[index] = true;
          }
        }
      }
    }
    assert_eq!(accepted, [true, true], "kernel must distribute flows to both listeners");
    drop(clients);
  }
  #[cfg(not(target_os = "linux"))]
  assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::Unsupported));
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_listener_and_connect_complete_on_the_owner() {
  let mut driver = driver(8);
  let listener = driver.listen("127.0.0.1:0".parse().unwrap(), 16).unwrap();
  let accept = driver.accept(&listener).unwrap();
  let (client, connect) = driver.connect(listener.local_address().unwrap()).unwrap();
  let mut connected = false;
  let mut accepted = None;
  for _ in 0..2 {
    match next(&mut driver) {
      Event::Connected { id, result } => {
        assert_eq!(id, connect);
        result.unwrap();
        connected = true;
      }
      Event::Accepted { id, result } => {
        assert_eq!(id, accept);
        accepted = Some(result.unwrap());
      }
      event => panic!("unexpected event: {event:?}"),
    }
  }
  assert!(connected);
  let server = driver.attach(accepted.unwrap()).unwrap();
  assert_eq!(client.local_address().unwrap(), server.remote_address().unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn shutdown_drains_pending_receive_and_rejects_new_work() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(32);
  driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  assert!(driver.try_shutdown(Duration::from_secs(2)).unwrap());
  assert_eq!(budget.used(), 0);
  assert!(driver.receive(&connection, Buffer::new(32, budget).unwrap()).is_err());
}

fn next(driver: &mut Driver) -> Event {
  let deadline = Instant::now() + Duration::from_secs(5);
  loop {
    if let Some(event) = driver.poll(Duration::from_millis(10), 1).unwrap().pop() {
      return event;
    }
    assert!(Instant::now() < deadline, "operation timed out");
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn bounded_operations_return_rejected_storage_and_cancel_exactly_once() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut driver = driver(1);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(64);
  let id = driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  let (error, rejected) = driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap_err();
  assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
  assert_eq!(budget.used(), 64);
  drop(rejected);
  assert_eq!(budget.used(), 32);
  assert!(driver.poll(Duration::ZERO, 0).is_err());
  assert!(driver.poll(Duration::ZERO, 2).is_err());
  assert!(driver.cancel(id));
  let completed = next(&mut driver);
  assert!(matches!(&completed, Event::Received { id: found, result: Err(_), .. } if *found == id));
  assert!(!driver.cancel(id));
  assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
  drop(completed);
  assert_eq!(budget.used(), 0);
  let next_id = driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  assert_ne!(id, next_id);
  drop(connection);
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn fragmented_receives_preserve_order_and_eof_after_peer_half_close() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(1);
  peer.write_all(b"fragmented").unwrap();
  peer.shutdown(std::net::Shutdown::Write).unwrap();
  for expected in b"fragmented" {
    let id = driver
      .receive(&connection, Buffer::new(1, budget.clone()).unwrap())
      .unwrap();
    let Event::Received {
      id: found,
      result,
      buffer,
    } = next(&mut driver)
    else {
      panic!("receive expected")
    };
    assert_eq!(found, id);
    assert_eq!(result.unwrap(), 1);
    assert_eq!(buffer.freeze().as_ref(), &[*expected]);
    assert_eq!(budget.used(), 0);
  }
  driver
    .receive(&connection, Buffer::new(1, budget.clone()).unwrap())
    .unwrap();
  let Event::Received { result, buffer, .. } = next(&mut driver) else {
    panic!("EOF expected")
  };
  assert_eq!(result.unwrap(), 0);
  drop(buffer);
  assert_eq!(budget.used(), 0);
}

fn driver(limit: usize) -> Driver {
  let requested = common::backend();
  let driver = Driver::new(requested, limit).expect("requested backend unavailable");
  assert_ne!(driver.backend(), Backend::Auto);
  if requested != Backend::Auto {
    assert_eq!(driver.backend(), requested, "backend silently fell back");
  }
  driver
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn operation_quota_includes_other_sockets_and_batches_are_bounded() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let mut driver = driver(2);
  let mut connections = Vec::new();
  let mut peers = Vec::new();
  let budget = Budget::new(96);
  let mut ids = Vec::new();
  for _ in 0..2 {
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peers.push(listener.accept().unwrap().0);
    let connection = driver.attach(client.into()).unwrap();
    ids.push(
      driver
        .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
        .unwrap(),
    );
    connections.push(connection);
  }
  let mut output = Buffer::new(32, budget.clone()).unwrap();
  output.write(0, b"quota").unwrap();
  let (error, rejected) = driver.send(&connections[0], output.freeze()).unwrap_err();
  assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
  drop(rejected);
  assert_eq!(budget.used(), 64);
  for id in &ids {
    assert!(driver.cancel(*id));
  }
  for _ in 0..2 {
    let Event::Received { id, result, buffer } = next(&mut driver) else {
      panic!("cancelled receive expected")
    };
    let index = ids
      .iter()
      .position(|expected| *expected == id)
      .expect("unique completion");
    ids.remove(index);
    assert!(result.is_err());
    drop(buffer);
  }
  assert!(ids.is_empty());
  assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn large_send_resubmits_unwritten_slices_and_retains_storage_after_teardown() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  connection.set_option(4, 4096).unwrap();
  let size = 512 * 1024 + 17;
  let budget = Budget::new(size);
  let expected: Vec<u8> = (0..size).map(|offset| offset as u8).collect();
  let mut buffer = Buffer::new(size, budget.clone()).unwrap();
  buffer.write(0, &expected).unwrap();
  let retained = buffer.freeze();
  let peer = std::thread::spawn(move || {
    let mut received = vec![0; size];
    peer.read_exact(&mut received).unwrap();
    assert_eq!(received, expected);
  });
  let mut offset = 0;
  while offset < size {
    let submitted = driver.send(&connection, retained.slice(offset..size).unwrap()).unwrap();
    let Event::Sent { id, result, buffer } = next(&mut driver) else {
      panic!("send expected")
    };
    assert_eq!(id, submitted);
    let written = result.unwrap();
    assert!(written > 0 && written <= size - offset);
    offset += written;
    drop(buffer);
  }
  peer.join().unwrap();
  drop(connection);
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  drop(driver);
  assert_eq!(budget.used(), size);
  assert_eq!(retained.as_ref()[size - 1], (size - 1) as u8);
  drop(retained);
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn borrowed_batches_drop_undelivered_events_without_releasing_retained_storage() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let mut driver = driver(3);
  let budget = Budget::new(96);
  let mut peers = Vec::new();
  let mut connections = Vec::new();
  for _ in 0..3 {
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peers.push(listener.accept().unwrap().0);
    let connection = driver.attach(client.into()).unwrap();
    let id = driver
      .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
      .unwrap();
    assert!(driver.cancel(id));
    connections.push(connection);
  }
  let mut retained = None;
  let mut delivered = 0;
  let deadline = Instant::now() + Duration::from_secs(5);
  while driver.outstanding() != 0 {
    let mut batch = driver.poll_batch(Duration::from_millis(10), 1).unwrap();
    assert!(batch.len() <= 1);
    delivered += batch.len();
    if retained.is_none() {
      retained = batch.next();
    }
    // Dropping a partially consumed batch must reclaim only its selected events.
    drop(batch);
    assert!(Instant::now() < deadline, "cancelled batches did not drain");
  }
  assert_eq!(delivered, 3);
  assert!(matches!(retained, Some(Event::Received { result: Err(_), .. })));
  assert_eq!(budget.used(), 32);
  assert!(driver.poll_batch(Duration::ZERO, 1).unwrap().next().is_none());
  drop(driver);
  assert_eq!(budget.used(), 32);
  drop(retained);
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn receive_after_cancellation_registers_the_source_again() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(64);
  let cancelled = driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  assert!(driver.cancel(cancelled));
  let Event::Received { id, result, .. } = next(&mut driver) else {
    panic!("cancellation expected")
  };
  assert_eq!(id, cancelled);
  assert!(result.is_err());
  // The source lost its only interest; a later receive has to arm it again.
  let receive = driver.receive(&connection, Buffer::new(32, budget).unwrap()).unwrap();
  peer.write_all(b"again").unwrap();
  let Event::Received { id, result, buffer } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(id, receive);
  assert_eq!(result.unwrap(), 5);
  assert_eq!(&buffer.freeze().as_ref()[..5], b"again");
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn bytes_arriving_while_reads_are_paused_survive_the_idle_source() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(64);
  driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  peer.write_all(b"first").unwrap();
  let Event::Received { result, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(result.unwrap(), 5);
  // No read is pending: the source is idle while more bytes arrive, and the
  // driver must neither spin on them nor lose them.
  peer.write_all(b"second").unwrap();
  let start = Instant::now();
  assert!(
    driver.poll(Duration::from_millis(50), 1).unwrap().is_empty(),
    "an idle source produced an event"
  );
  assert!(start.elapsed() >= Duration::from_millis(40), "idle poll returned early");
  driver.receive(&connection, Buffer::new(32, budget).unwrap()).unwrap();
  let Event::Received { result, buffer, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(result.unwrap(), 6);
  assert_eq!(&buffer.freeze().as_ref()[..6], b"second");
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn a_reused_descriptor_number_is_registered_again() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let address = listener.local_addr().unwrap();
  let mut driver = driver(8);
  let budget = Budget::new(64);

  let first = TcpStream::connect(address).unwrap();
  let number = first.as_raw_fd();
  let (mut peer, _) = listener.accept().unwrap();
  let connection = driver.attach(first.into()).unwrap();
  driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  peer.write_all(b"one").unwrap();
  let Event::Received { result, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(result.unwrap(), 3);
  drop(connection);
  drop(peer);

  // Reserve unconnected sockets so forcing fd reuse cannot fill the listener backlog.
  let mut held = Vec::new();
  let deadline = Instant::now() + Duration::from_secs(5);
  let second = loop {
    let candidate = socket2::Socket::new(
      socket2::Domain::IPV4,
      socket2::Type::STREAM,
      Some(socket2::Protocol::TCP),
    )
    .unwrap();
    if candidate.as_raw_fd() == number {
      break candidate;
    }
    if candidate.as_raw_fd() < number {
      held.push(candidate);
    } else {
      // A parallel test can temporarily own the old number; leave later numbers available.
      drop(candidate);
      std::thread::yield_now();
    }
    assert!(Instant::now() < deadline, "the descriptor number was never reused");
  };
  second.connect(&address.into()).unwrap();
  held.clear();
  let (mut peer, _) = listener.accept().unwrap();
  let connection = driver.attach(second).unwrap();
  driver.receive(&connection, Buffer::new(32, budget).unwrap()).unwrap();
  peer.write_all(b"two").unwrap();
  let Event::Received { result, buffer, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(result.unwrap(), 3);
  assert_eq!(&buffer.freeze().as_ref()[..3], b"two");
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn a_receive_submitted_against_stale_readiness_still_sees_buffered_bytes() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(128);
  driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  peer.write_all(b"one").unwrap();
  let Event::Received { result, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(result.unwrap(), 3);
  // The driver now treats the source as spent. These bytes land before the next
  // receive is submitted and with no poll in between, so only re-registration can
  // recover them.
  peer.write_all(b"two").unwrap();
  peer.flush().unwrap();
  driver.receive(&connection, Buffer::new(32, budget).unwrap()).unwrap();
  let Event::Received { result, buffer, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  assert_eq!(result.unwrap(), 3);
  assert_eq!(&buffer.freeze().as_ref()[..3], b"two");
}

fn view(bytes: &[u8], budget: &Budget) -> FrozenBuffer {
  let mut buffer = Buffer::new(bytes.len(), budget.clone()).unwrap();
  buffer.write(0, bytes).unwrap();
  buffer.freeze()
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn vectored_send_reaches_the_peer_concatenated_in_order() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(14);
  let views = vec![view(b"alpha", &budget), view(b"beta", &budget), view(b"gamma", &budget)];
  let send = driver.send_vectored(&connection, views).unwrap();
  let Event::SentVectored {
    id,
    result,
    mut buffers,
  } = next(&mut driver)
  else {
    panic!("vectored send expected")
  };
  assert_eq!(id, send);
  assert_eq!(result.unwrap(), 14);
  assert_eq!(buffers.len(), 3);
  assert!(buffers.advance(14).unwrap());
  assert!(buffers.is_empty());
  assert_eq!(budget.used(), 0);
  let mut reply = [0; 14];
  peer.read_exact(&mut reply).unwrap();
  assert_eq!(&reply, b"alphabetagamma");
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn a_large_vectored_send_resumes_from_its_reported_count() {
  const PART: usize = 256 * 1024;
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  // Force short sendmsg progress on Unix; overlapped WSASend may consume the entire vector.
  connection.set_option(4, 4096).unwrap();
  let budget = Budget::new(3 * PART);
  let mut expected = Vec::with_capacity(3 * PART);
  let mut views = Vec::with_capacity(3);
  for byte in *b"abc" {
    let part = vec![byte; PART];
    expected.extend_from_slice(&part);
    views.push(view(&part, &budget));
  }
  let reader = std::thread::spawn(move || {
    let mut received = Vec::new();
    peer.read_to_end(&mut received).unwrap();
    received
  });
  let mut sends = 0;
  loop {
    let id = driver.send_vectored(&connection, views).unwrap();
    sends += 1;
    let Event::SentVectored {
      id: found,
      result,
      buffers,
    } = next(&mut driver)
    else {
      panic!("vectored send expected")
    };
    assert_eq!(found, id);
    let sent = result.unwrap();
    views = buffers;
    let queued: usize = views.iter().map(|view| view.as_ref().len()).sum();
    if views.advance(sent).unwrap() {
      assert_eq!(sent, queued);
      break;
    }
    assert!(sent < queued, "a resumable send must report short progress");
  }
  if cfg!(unix) {
    assert!(sends > 1, "the send was never short enough to resume");
  }
  assert_eq!(budget.used(), 0);
  connection.shutdown(std::net::Shutdown::Write).unwrap();
  assert_eq!(reader.join().unwrap(), expected);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn a_rejected_vectored_send_returns_every_view_to_the_caller() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut driver = driver(1);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(64);
  driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  let views = vec![view(b"one", &budget), view(b"two", &budget)];
  let (error, rejected) = driver.send_vectored(&connection, views).unwrap_err();
  assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
  assert_eq!(rejected.len(), 2);
  assert_eq!(rejected[0].as_ref(), b"one");
  assert_eq!(rejected[1].as_ref(), b"two");
  assert_eq!(budget.used(), 38);
  drop(rejected);
  assert_eq!(budget.used(), 32);
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn a_cancelled_vectored_send_still_returns_its_views() {
  const PART: usize = 256 * 1024;
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client.into()).unwrap();
  connection.set_option(4, 4096).unwrap();
  let budget = Budget::new(3 * PART);
  // The peer never reads, so this first send fills both buffers and leaves the socket blocked.
  let filler = view(&vec![b'f'; 2 * PART], &budget);
  driver.send(&connection, filler).unwrap();
  let Event::Sent { result, buffer, .. } = next(&mut driver) else {
    panic!("send expected")
  };
  result.unwrap();
  drop(buffer);
  let views = vec![view(b"one", &budget), view(b"two", &budget)];
  let id = driver.send_vectored(&connection, views).unwrap();
  driver.cancel(id);
  let Event::SentVectored { id: found, buffers, .. } = next(&mut driver) else {
    panic!("vectored send expected")
  };
  assert_eq!(found, id);
  assert_eq!(buffers.len(), 2);
  assert!(!driver.cancel(id));
  drop(buffers);
  assert_eq!(budget.used(), 0);
}

#[test]
fn resuming_a_vectored_send_cannot_stall_or_overrun() {
  let budget = Budget::new(14);
  let mut views = vec![view(b"alpha", &budget), view(b"beta", &budget), view(b"gamma", &budget)];
  assert_eq!(views.advance(0).unwrap_err().kind(), std::io::ErrorKind::WriteZero);
  assert_eq!(views.advance(15).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
  assert!(!views.advance(11).unwrap());
  assert_eq!(views.len(), 1);
  assert_eq!(views[0].as_ref(), b"mma");
  assert!(!views.advance(1).unwrap());
  assert_eq!(views[0].as_ref(), b"ma");
  assert!(views.advance(2).unwrap());
  assert!(views.is_empty());
  assert!(views.advance(0).unwrap());
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn zero_timeout_poll_progresses_without_new_submissions() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let mut driver = driver(4);
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(32);
  let id = driver
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap();
  assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
  peer.write_all(b"progress").unwrap();
  let deadline = Instant::now() + Duration::from_secs(5);
  loop {
    let mut events = driver.poll(Duration::ZERO, 1).unwrap();
    if let Some(Event::Received {
      id: found,
      result,
      buffer,
    }) = events.pop()
    {
      assert_eq!(found, id);
      assert_eq!(result.unwrap(), 8);
      assert_eq!(buffer.freeze().as_ref(), b"progress");
      break;
    }
    assert!(Instant::now() < deadline, "no progress without a new SQE");
    std::thread::yield_now();
  }
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn busy_connection_does_not_starve_receive_or_cancellation() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let mut driver = driver(8);
  let budget = Budget::new(96);
  let mut connections = Vec::new();
  let mut peers = Vec::new();
  let mut ids = Vec::new();
  for _ in 0..3 {
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peers.push(listener.accept().unwrap().0);
    let connection = driver.attach(client.into()).unwrap();
    ids.push(
      driver
        .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
        .unwrap(),
    );
    connections.push(connection);
  }
  assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
  peers[0].write_all(&[b'a'; 4096]).unwrap();
  peers[1].write_all(b"cold").unwrap();
  assert!(driver.cancel(ids[2]));
  let mut cold = false;
  let mut cancelled = false;
  let mut busy = 0;
  let deadline = Instant::now() + Duration::from_secs(5);
  while !cold || !cancelled || busy == 0 {
    for event in driver.poll(Duration::ZERO, 1).unwrap() {
      let Event::Received { id, result, buffer } = event else {
        panic!("receive expected")
      };
      if id == ids[1] {
        assert!(!cold);
        assert_eq!(result.unwrap(), 4);
        assert_eq!(buffer.freeze().as_ref(), b"cold");
        cold = true;
      } else if id == ids[2] {
        assert!(!cancelled);
        assert!(result.is_err());
        cancelled = true;
      } else {
        assert_eq!(id, ids[0]);
        assert!(result.unwrap() > 0);
        busy += 1;
        drop(buffer);
        peers[0].write_all(&[b'a'; 32]).unwrap();
        ids[0] = driver
          .receive(&connections[0], Buffer::new(32, budget.clone()).unwrap())
          .unwrap();
      }
    }
    assert!(Instant::now() < deadline, "busy socket starved independent work");
    std::thread::yield_now();
  }
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn another_driver_cannot_submit_on_the_owners_connection() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut owner = driver(4);
  let connection = owner.attach(client.into()).unwrap();
  let mut other = driver(4);
  let budget = Budget::new(32);
  let (error, buffer) = other
    .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
    .unwrap_err();
  assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
  assert_eq!(other.outstanding(), 0);
  drop(buffer);
  assert_eq!(budget.used(), 0);
  assert!(owner.try_shutdown(Duration::ZERO).unwrap());
  assert!(other.try_shutdown(Duration::ZERO).unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn idle_driver_reports_resolved_setup_and_shuts_down_without_socket_activity() {
  let mut driver = driver(4);
  if driver.backend() == Backend::IoUring {
    for flag in ["SINGLE_ISSUER", "DEFER_TASKRUN", "TASKRUN_FLAG"] {
      assert!(driver.backend_description().contains(flag));
    }
  } else {
    assert!(!driver.backend_description().is_empty());
  }
  assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
  assert!(driver.try_shutdown(Duration::ZERO).unwrap());
  assert!(driver.try_shutdown(Duration::ZERO).unwrap());
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn persistent_keepalive_reuses_receive_registration_and_fixed_send_state() {
  let mut driver = driver(8);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(256 * 1024);
  let receive = driver
    .enable_persistent_receive(&connection, budget.clone(), 16384, 256 * 1024)
    .unwrap();
  for round in 0..64 {
    peer.write_all(b"request").unwrap();
    let Event::PersistentReceived {
      id,
      result,
      buffer,
      credit,
      ..
    } = next(&mut driver)
    else {
      panic!("persistent receive expected")
    };
    assert_eq!(id, receive);
    assert_eq!(result.unwrap(), 7);
    let body = buffer.unwrap().freeze();
    assert_eq!(body.as_ref(), b"request");
    credit.unwrap().ack();
    let send = match round % 3 {
      0 => driver.send(&connection, body).unwrap(),
      1 => driver.send_vectored(&connection, vec![body]).unwrap(),
      _ => driver
        .send_vectored(
          &connection,
          vec![
            FrozenBuffer::slice(&body, 0..3).unwrap(),
            FrozenBuffer::slice(&body, 3..7).unwrap(),
          ],
        )
        .unwrap(),
    };
    match next(&mut driver) {
      Event::Sent { id, result, buffer } if round % 3 == 0 => {
        assert_eq!(id, send);
        assert_eq!(result.unwrap(), 7);
        assert_eq!(buffer.as_ref(), b"request");
      }
      Event::SentVectored { id, result, buffers } if round % 3 != 0 => {
        assert_eq!(id, send);
        assert_eq!(result.unwrap(), 7);
        assert_eq!(buffers.len(), if round % 3 == 1 { 1 } else { 2 });
        let bytes: Vec<_> = buffers.iter().flat_map(|b| b.as_ref().iter().copied()).collect();
        assert_eq!(bytes, b"request");
      }
      event => panic!("unexpected send completion: {event:?}"),
    }
    let mut response = [0; 7];
    peer.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"request");
  }
  let stats = driver.persistent_stats().unwrap();
  assert_eq!(stats.receive_registrations, 1);
  assert_eq!(stats.send_completions, 64);
  assert_eq!(stats.buffer_exhaustions, 0);
  assert!(driver.cancel(receive));
  loop {
    if let Event::PersistentRetired { id } = next(&mut driver) {
      assert_eq!(id, receive);
      break;
    }
  }
  assert_eq!(budget.used(), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn persistent_pause_resume_preserves_bytes_and_retained_storage_after_shutdown() {
  let mut driver = driver(8);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(256 * 1024);
  let receive = driver
    .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
    .unwrap();
  peer.write_all(b"retained").unwrap();
  let Event::PersistentReceived { buffer, credit, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  let retained = buffer.unwrap().freeze();
  credit.unwrap().ack();
  driver.pause_persistent_receive(receive).unwrap();
  // Observe target termination without retiring the logical registration.
  let deadline = Instant::now() + Duration::from_secs(5);
  while driver.persistent_stats().unwrap().terminal_completions == 0 {
    assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
    assert!(Instant::now() < deadline);
  }
  peer.write_all(b"resumed").unwrap();
  assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
  driver.resume_persistent_receive(receive).unwrap();
  let Event::PersistentReceived {
    id,
    result,
    buffer,
    credit,
    ..
  } = next(&mut driver)
  else {
    panic!("resumed receive expected")
  };
  assert_eq!(id, receive);
  assert_eq!(result.unwrap(), 7);
  assert_eq!(buffer.unwrap().freeze().as_ref(), b"resumed");
  drop(credit);
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  drop(driver);
  assert_eq!(retained.as_ref(), b"retained");
  assert_eq!(budget.used(), 16384);
  std::thread::spawn(move || drop(retained)).join().unwrap();
  assert_eq!(budget.used(), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn credit_paused_receive_resumes_while_old_view_is_retained() {
  let mut driver = driver(8);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(65536);
  let id = driver
    .enable_persistent_receive(&connection, budget.clone(), 16384, 16384)
    .unwrap();
  peer.write_all(b"A").unwrap();
  let Event::PersistentReceived { buffer, credit, .. } = next(&mut driver) else {
    panic!("first receive expected")
  };
  let retained = buffer.unwrap().freeze();
  peer.write_all(b"B").unwrap();
  let deadline = Instant::now() + Duration::from_secs(5);
  while driver.persistent_stats().unwrap().terminal_completions == 0 {
    assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
    assert!(Instant::now() < deadline, "expected credit-paused receive retirement");
  }
  // No further peer writes: acknowledgement alone must wake and rearm the terminal receive.
  credit.unwrap().ack();
  let Event::PersistentReceived {
    id: found,
    result,
    buffer,
    credit,
    ..
  } = next(&mut driver)
  else {
    panic!("rearmed receive expected")
  };
  assert_eq!(found, id);
  assert_eq!(result.unwrap(), 1);
  assert_eq!(buffer.unwrap().freeze().as_ref(), b"B");
  drop(credit);
  assert_eq!(retained.as_ref(), b"A");
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  assert_eq!(budget.used(), 16384);
  drop(retained);
  assert_eq!(budget.used(), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn queued_receive_groups_prevent_sleep_before_their_first_submission() {
  let mut driver = driver(512);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let budget = Budget::new(16 * 1024 * 1024);
  let mut connections = Vec::new();
  let mut peers = Vec::new();
  let mut last = 0;
  for _ in 0..257 {
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peers.push(listener.accept().unwrap().0);
    let connection = driver.attach(client.into()).unwrap();
    last = driver
      .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
      .unwrap();
    connections.push(connection);
  }
  assert_eq!(budget.used(), 32768, "idle sockets share two owner payload slots");
  peers.last_mut().unwrap().write_all(b"last").unwrap();
  let wake = driver.waker();
  let (done, waiting) = std::sync::mpsc::channel();
  let watchdog = std::thread::spawn(move || {
    if waiting.recv_timeout(Duration::from_secs(2)).is_err() {
      wake.wake();
      false
    } else {
      true
    }
  });
  let first = driver.poll(Duration::MAX, 1).unwrap();
  let _ = done.send(());
  assert!(
    watchdog.join().unwrap(),
    "slept with unsubmitted groups in the owner bitmap"
  );
  let event = first.into_iter().next().unwrap_or_else(|| next(&mut driver));
  let Event::PersistentReceived {
    id,
    result,
    buffer,
    credit,
    ..
  } = event
  else {
    panic!("last group receive expected")
  };
  assert_eq!(id, last);
  assert_eq!(result.unwrap(), 4);
  drop(buffer);
  drop(credit);
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  assert_eq!(budget.used(), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn persistent_close_with_read_and_vectored_write_active_retires_once() {
  let mut driver = driver(8);
  if driver.backend() != Backend::IoUring {
    return;
  }
  for parts in [1, 2] {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let connection = driver.attach(client.into()).unwrap();
    connection.set_option(4, 4096).unwrap();
    let budget = Budget::new(2 * 1024 * 1024);
    let receive = driver
      .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
      .unwrap();
    assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
    let buffers = (0..parts)
      .map(|_| view(&vec![b'a'; 1024 * 1024 / parts], &budget))
      .collect();
    let send = driver.send_vectored(&connection, buffers).unwrap();
    let peer = socket2::Socket::from(peer);
    peer.set_linger(Some(Duration::ZERO)).unwrap();
    drop(peer);
    assert!(driver.cancel(receive));
    let mut sent = false;
    loop {
      match next(&mut driver) {
        Event::SentVectored { id, buffers, .. } => {
          assert_eq!(id, send);
          assert!(!sent);
          assert_eq!(
            buffers.iter().map(|buffer| buffer.as_ref().len()).sum::<usize>(),
            1024 * 1024
          );
          sent = true;
        }
        Event::PersistentRetired { id } => {
          assert_eq!(id, receive);
          break;
        }
        event => panic!("unexpected event during close: {event:?}"),
      }
    }
    assert!(sent);
    assert!(!driver.cancel(receive));
    assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
    assert_eq!(budget.used(), 0);
  }
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn persistent_large_sends_retire_notifications_before_releasing_storage() {
  for vectored in [false, true] {
    let mut driver = driver(8);
    if driver.backend() != Backend::IoUring {
      return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let connection = driver.attach(client.into()).unwrap();
    connection.set_option(4, 4096).unwrap();
    let budget = Budget::new(2 * 1024 * 1024);
    let receive = driver
      .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
      .unwrap();
    let reader = std::thread::spawn(move || {
      let mut bytes = vec![0; 256 * 1024];
      peer.read_exact(&mut bytes).unwrap();
      assert!(bytes[..128 * 1024].iter().all(|byte| *byte == b'a'));
      assert!(bytes[128 * 1024..].iter().all(|byte| *byte == b'b'));
      peer
    });
    for byte in *b"ab" {
      let mut buffers = if vectored {
        vec![view(&vec![byte; 65536], &budget), view(&vec![byte; 65536], &budget)]
      } else {
        vec![view(&vec![byte; 131072], &budget)]
      };
      while !buffers.is_empty() {
        let id = if vectored {
          driver.send_vectored(&connection, buffers).unwrap()
        } else {
          driver.send(&connection, buffers.pop().unwrap()).unwrap()
        };
        let (sent, returned) = match next(&mut driver) {
          Event::Sent {
            id: done,
            result,
            buffer,
          } => {
            assert_eq!(done, id);
            (result.unwrap(), vec![buffer])
          }
          Event::SentVectored {
            id: done,
            result,
            buffers,
          } => {
            assert_eq!(done, id);
            (result.unwrap(), buffers)
          }
          event => panic!("unexpected event: {event:?}"),
        };
        assert!(sent > 0);
        let mut consumed = sent;
        buffers = returned
          .into_iter()
          .filter_map(|buffer| {
            let length = buffer.as_ref().len();
            if consumed >= length {
              consumed -= length;
              None
            } else {
              let tail = FrozenBuffer::slice(&buffer, consumed..length).unwrap();
              consumed = 0;
              Some(tail)
            }
          })
          .collect();
        assert_eq!(consumed, 0);
      }
    }
    let peer = reader.join().unwrap();
    assert!(driver.cancel(receive));
    loop {
      if let Event::PersistentRetired { id } = next(&mut driver) {
        assert_eq!(id, receive);
        break;
      }
    }
    let stats = driver.persistent_stats().unwrap();
    if stats.zero_copy_supported {
      assert!(stats.zero_copy_sends > 0);
      assert!(stats.zero_copy_notifications > 0 || stats.zero_copy_fallbacks > 0);
    }
    assert_eq!(stats.zero_copy_pending_bytes, 0);
    assert_eq!(budget.used(), 0);
    drop(peer);
  }
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn retired_fixed_slot_can_serve_a_new_socket_without_old_completions() {
  let mut driver = driver(4);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let budget = Budget::new(65536);
  for byte in 0..16_u8 {
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    let connection = driver.attach(client.into()).unwrap();
    let receive = driver
      .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
      .unwrap();
    peer.write_all(&[byte]).unwrap();
    let Event::PersistentReceived { id, buffer, credit, .. } = next(&mut driver) else {
      panic!("receive expected")
    };
    assert_eq!(id, receive);
    assert_eq!(buffer.unwrap().freeze().as_ref(), &[byte]);
    drop(credit);
    assert!(driver.cancel(receive));
    let Event::PersistentRetired { id } = next(&mut driver) else {
      panic!("retirement expected")
    };
    assert_eq!(id, receive);
    assert_eq!(budget.used(), 0);
    assert!(driver.poll(Duration::ZERO, 1).unwrap().is_empty());
  }
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn persistent_receive_at_socket_limit_still_admits_its_response() {
  let mut driver = driver(1);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  let connection = driver.attach(client.into()).unwrap();
  let budget = Budget::new(65536);
  let receive = driver
    .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
    .unwrap();
  peer.write_all(b"x").unwrap();
  let Event::PersistentReceived { buffer, credit, .. } = next(&mut driver) else {
    panic!("receive expected")
  };
  credit.unwrap().ack();
  assert_eq!(driver.outstanding(), 1);
  let send = driver.send(&connection, buffer.unwrap().freeze()).unwrap();
  let Event::Sent { id, result, buffer } = next(&mut driver) else {
    panic!("response expected")
  };
  assert_eq!(id, send);
  assert_eq!(result.unwrap(), 1);
  drop(buffer);
  let mut byte = [0];
  peer.read_exact(&mut byte).unwrap();
  assert_eq!(&byte, b"x");
  assert!(driver.cancel(receive));
  let Event::PersistentRetired { id } = next(&mut driver) else {
    panic!("retirement expected")
  };
  assert_eq!(id, receive);
  assert_eq!(budget.used(), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn saturated_mixed_operations_keep_cancellation_and_storage_bounded() {
  let mut driver = driver(4);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let budget = Budget::new(128 * 1024);
  let mut connections = Vec::new();
  let mut peers = Vec::new();
  let mut receives = Vec::new();
  for index in 0..4 {
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peers.push(listener.accept().unwrap().0);
    let connection = driver.attach(client.into()).unwrap();
    let id = if index < 2 {
      driver
        .enable_persistent_receive(&connection, budget.clone(), 16384, 32768)
        .unwrap()
    } else {
      driver
        .receive(&connection, Buffer::new(32, budget.clone()).unwrap())
        .unwrap()
    };
    receives.push(id);
    connections.push(connection);
  }
  let early = driver.poll(Duration::ZERO, 4).unwrap();
  assert!(early.is_empty(), "no operation should complete yet: {early:?}");
  let mut sends = Vec::new();
  for connection in &connections[..2] {
    sends.push(driver.send(connection, view(b"ok", &budget)).unwrap());
  }
  let (error, rejected) = driver.send(&connections[2], view(b"full", &budget)).unwrap_err();
  assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
  drop(rejected);
  for id in &receives {
    assert!(driver.cancel(*id));
  }
  let deadline = Instant::now() + Duration::from_secs(5);
  while !receives.is_empty() || !sends.is_empty() {
    for event in driver.poll(Duration::ZERO, 4).unwrap() {
      let (ids, id) = match event {
        Event::PersistentRetired { id } | Event::Received { id, .. } => (&mut receives, id),
        Event::Sent { id, .. } => (&mut sends, id),
        event => panic!("unexpected event during mixed shutdown: {event:?}"),
      };
      let index = ids
        .iter()
        .position(|expected| *expected == id)
        .expect("exactly one terminal result");
      ids.remove(index);
    }
    assert!(Instant::now() < deadline, "mixed cancellation did not retire");
  }
  assert!(driver.try_shutdown(Duration::ZERO).unwrap());
  assert_eq!(budget.used(), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn shared_receive_pool_keeps_other_connections_live_while_one_retains_credit() {
  let mut driver = driver(8);
  if driver.backend() != Backend::IoUring {
    return;
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let budget = Budget::new(1024 * 1024);
  let mut connections = Vec::new();
  let mut peers = Vec::new();
  let mut ids = Vec::new();
  for _ in 0..2 {
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peers.push(listener.accept().unwrap().0);
    let connection = driver.attach(client.into()).unwrap();
    ids.push(
      driver
        .enable_persistent_receive(&connection, budget.clone(), 16384, 16384)
        .unwrap(),
    );
    connections.push(connection);
  }
  assert_eq!(budget.used(), 32768);
  peers[0].write_all(b"A").unwrap();
  let Event::PersistentReceived { id, buffer, credit, .. } = next(&mut driver) else {
    panic!("first connection receive expected");
  };
  assert_eq!(id, ids[0]);
  let retained = buffer.unwrap().freeze();
  peers[0].write_all(b"B").unwrap();
  peers[0].shutdown(std::net::Shutdown::Write).unwrap();
  peers[1].write_all(b"C").unwrap();
  let Event::PersistentReceived {
    id,
    buffer,
    credit: other_credit,
    ..
  } = next(&mut driver)
  else {
    panic!("unblocked connection receive expected");
  };
  assert_eq!(id, ids[1], "paused connection cannot bypass delivery credit");
  assert_eq!(buffer.unwrap().freeze().as_ref(), b"C");
  drop(other_credit);
  credit.unwrap().ack();
  let Event::PersistentReceived { id, buffer, credit, .. } = next(&mut driver) else {
    panic!("resumed connection receive expected");
  };
  assert_eq!(id, ids[0]);
  assert_eq!(buffer.unwrap().freeze().as_ref(), b"B");
  drop(credit);
  let Event::PersistentReceived { id, result, .. } = next(&mut driver) else {
    panic!("ordered EOF expected");
  };
  assert_eq!(id, ids[0]);
  assert_eq!(result.unwrap(), 0);
  assert_eq!(retained.as_ref(), b"A");
  assert!(driver.try_shutdown(Duration::from_secs(5)).unwrap());
  assert_eq!(budget.used(), 16384);
  drop(retained);
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn driver_bounds_and_platform_backends_are_validated() {
  for limit in [0, 65537] {
    for backend in [Backend::Auto, Backend::Polling] {
      let error = Driver::new(backend, limit).err().expect("bounds are validated");
      assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{limit}");
    }
  }
  #[cfg(not(windows))]
  {
    let error = Driver::new(Backend::Iocp, 8).err().expect("IOCP is Windows-only");
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
  }
}

#[cfg(not(windows))]
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn polling_drivers_report_themselves_and_reject_persistent_receives() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
  let (_peer, _) = listener.accept().unwrap();
  let mut driver = Driver::new(Backend::Polling, 4).unwrap();
  assert_eq!(driver.backend(), Backend::Polling);
  assert_eq!(driver.backend_description(), "polling");
  assert!(driver.persistent_stats().is_none());
  let connection = driver.attach(client.into()).unwrap();
  let error = driver
    .enable_persistent_receive(&connection, Budget::new(1 << 16), 4096, 1 << 16)
    .unwrap_err();
  assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
  for result in [driver.pause_persistent_receive(1), driver.resume_persistent_receive(1)] {
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
  }
  assert!(driver.try_shutdown(Duration::ZERO).unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn listeners_reject_negative_backlogs_and_closed_drivers_reject_sockets() {
  let mut first = driver(4);
  let address = "127.0.0.1:0".parse().unwrap();
  assert_eq!(
    first.listen(address, -1).err().unwrap().kind(),
    std::io::ErrorKind::InvalidInput
  );
  assert_eq!(
    first.listen_configured(address, -1, false).err().unwrap().kind(),
    std::io::ErrorKind::InvalidInput
  );
  #[cfg(target_os = "linux")]
  assert_eq!(
    first.listen_sharded(address, -1).err().unwrap().kind(),
    std::io::ErrorKind::InvalidInput
  );
  let listener = first.listen_configured(address, 4, false).unwrap();
  let mut other = driver(4);
  assert_eq!(
    other.accept(&listener).unwrap_err().kind(),
    std::io::ErrorKind::InvalidInput,
    "a listener belongs to one driver"
  );
  assert!(first.try_shutdown(Duration::ZERO).unwrap());
  let socket = TcpListener::bind("127.0.0.1:0").unwrap();
  assert_eq!(
    first.attach(socket.into()).err().unwrap().kind(),
    std::io::ErrorKind::BrokenPipe
  );
  assert_eq!(
    first.accept(&listener).unwrap_err().kind(),
    std::io::ErrorKind::BrokenPipe
  );
  assert!(other.try_shutdown(Duration::ZERO).unwrap());
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "real sockets are unavailable under miri")]
fn borrowed_vectors_handle_partial_writes_backpressure_and_owned_fallback() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let client = socket2::Socket::from(TcpStream::connect(listener.local_addr().unwrap()).unwrap());
  client.set_send_buffer_size(4096).unwrap();
  let (mut peer, _) = listener.accept().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  let mut driver = driver(8);
  let connection = driver.attach(client).unwrap();
  let left = vec![11; 65536];
  let right = vec![22; 65536];
  let regions = [std::io::IoSlice::new(&left), std::io::IoSlice::new(&right)];
  assert!(driver.try_send_vectored(&connection, &[]).is_err());
  let mut expected = Vec::new();
  let mut partial = false;
  let mut blocked = false;
  for _ in 0..1024 {
    match driver.try_send_vectored(&connection, &regions).unwrap() {
      Some(sent) if sent > 0 => {
        assert!(sent <= left.len() + right.len());
        partial |= sent < left.len() + right.len();
        expected.extend_from_slice(&left[..sent.min(left.len())]);
        if sent > left.len() {
          expected.extend_from_slice(&right[..sent - left.len()]);
        }
      }
      _ => {
        blocked = true;
        break;
      }
    }
  }
  assert!(blocked, "nonblocking writes must yield on backpressure");
  assert!(partial, "small send window exercises region advancement");
  assert_eq!(driver.outstanding(), 0, "borrowed sends create no completion or lease");
  let tail = vec![33; 2 * 1024 * 1024];
  let budget = Budget::new(tail.len());
  let mut storage = Buffer::new(tail.len(), budget.clone()).unwrap();
  storage.write(0, &tail).unwrap();
  let mut id = driver.send(&connection, storage.freeze()).unwrap();
  assert!(
    driver.try_send_vectored(&connection, &regions).unwrap().is_none(),
    "pending owned writes preserve order"
  );
  expected.extend_from_slice(&tail);
  let length = expected.len();
  let reader = std::thread::spawn(move || {
    let mut received = vec![0; length];
    peer.read_exact(&mut received).unwrap();
    received
  });
  let mut remaining = tail.len();
  loop {
    let Event::Sent {
      id: sent,
      result,
      buffer,
    } = next(&mut driver)
    else {
      panic!("owned fallback completion");
    };
    assert_eq!(sent, id);
    let count = result.unwrap();
    assert!(count > 0 && count <= remaining);
    remaining -= count;
    if remaining == 0 {
      drop(buffer);
      break;
    }
    let rest = buffer.slice(count..buffer.as_ref().len()).unwrap();
    drop(buffer);
    id = driver.send(&connection, rest).unwrap();
  }
  assert_eq!(reader.join().unwrap(), expected);
  drop(driver);
  assert_eq!(budget.used(), 0);
}
