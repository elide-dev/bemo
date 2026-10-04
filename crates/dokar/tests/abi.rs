mod common;

use dokar::abi::*;

#[test]
fn released_handles_cannot_alias_new_allocations() {
  let owner = elide_transport_owner_new(32);
  assert_ne!(owner, 0);
  let first = elide_transport_buffer_new(owner, 16);
  assert_ne!(first, 0);
  assert_eq!(elide_transport_owner_used(owner), 16);
  assert_eq!(elide_transport_buffer_release(first), 0);
  let second = elide_transport_buffer_new(owner, 16);
  assert_ne!(first, second);
  assert_eq!(elide_transport_buffer_release(first), INVALID);
  assert_eq!(elide_transport_owner_used(owner), 16);
  assert_eq!(elide_transport_buffer_release(second), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
fn freeze_and_slice_preserve_allocation_and_bounds() {
  let owner = elide_transport_owner_new(16);
  let buffer = elide_transport_buffer_new(owner, 16);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(buffer, &mut view) }, 0);
  assert_eq!(view.capacity, 16);
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(b"hello".as_ptr(), view.address.cast::<u8>(), 5) };
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(buffer, 17) }, INVALID);
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(buffer, 5) }, 0);
  let slice = elide_transport_buffer_slice(buffer, 1, 3);
  assert_ne!(slice, 0);
  assert_eq!(elide_transport_buffer_slice(buffer, u64::MAX, 3), 0);
  assert_eq!(elide_transport_buffer_release(buffer), 0);
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(slice, &mut view) }, 0);
  assert_eq!(view.length, 3);
  assert_eq!(view.flags, READ_ONLY);
  assert_eq!(
    // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
    unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), 3) },
    b"ell"
  );
  assert_eq!(elide_transport_owner_used(owner), 16);
  assert_eq!(elide_transport_owner_release(owner), 0);
  assert_eq!(elide_transport_buffer_new(owner, 1), 0);
  assert_eq!(elide_transport_buffer_release(slice), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn driver_handles_are_thread_affine_but_wakeup_is_not() {
  let handle = elide_transport_driver_new(common::workload(), common::backend() as u32, 8);
  assert_ne!(handle, 0);
  std::thread::spawn(move || {
    assert_eq!(elide_transport_driver_backend(handle), INVALID);
    assert_eq!(elide_transport_driver_wake(handle), 0);
    assert_eq!(elide_transport_driver_release(handle), INVALID);
  })
  .join()
  .unwrap();
  let actual = elide_transport_driver_backend(handle);
  assert!(actual > 0);
  if common::backend() as u32 != 0 {
    assert_eq!(actual, common::backend() as i32);
  }
  assert_eq!(elide_transport_driver_release(handle), 0);
  assert_eq!(elide_transport_driver_wake(handle), INVALID);
  assert_eq!(elide_transport_driver_release(handle), INVALID);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn driver_fallback_reports_the_refused_io_uring_setup() {
  let expected = std::env::var("ELIDE_TRANSPORT_TEST_EXPECT_FALLBACK").ok();
  let owner = elide_transport_owner_new(512);
  let output = elide_transport_buffer_new(owner, 256);
  let short = elide_transport_buffer_new(owner, 8);
  let driver = elide_transport_driver_new(owner, 0, 8);
  assert_ne!(driver, 0);
  let length = elide_transport_driver_fallback(driver, output);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(output, &mut view) }, 0);
  // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
  let reason = unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), length as usize) };
  let reason = std::str::from_utf8(reason).unwrap();
  match elide_transport_driver_backend(driver) {
    1 if cfg!(target_os = "linux") => assert!(length > 0, "polling AUTO must report why"),
    _ => assert_eq!(length, 0, "{reason}"),
  }
  if let Some(errno) = expected {
    assert!(reason.ends_with(&format!("(os error {errno})")), "{reason}");
  }
  let truncated = elide_transport_driver_fallback(driver, short);
  assert_eq!(truncated, length.min(8));
  std::thread::spawn(move || {
    assert_eq!(elide_transport_driver_fallback(driver, output), INVALID);
  })
  .join()
  .unwrap();
  if common::backend() as u32 != 0 {
    let forced = elide_transport_driver_new(owner, common::backend() as u32, 8);
    assert_eq!(elide_transport_driver_fallback(forced, output), 0);
    assert_eq!(elide_transport_driver_release(forced), 0);
  }
  assert_eq!(elide_transport_driver_release(driver), 0);
  assert_eq!(elide_transport_driver_fallback(driver, output), INVALID);
  assert_eq!(elide_transport_buffer_release(short), 0);
  assert_eq!(elide_transport_buffer_release(output), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(miri, ignore = "seccomp and io_uring are unavailable under miri")]
fn auto_driver_falls_back_when_the_kernel_rejects_ring_flags() {
  std::thread::spawn(|| {
    // Kernels before 6.1 reject SINGLE_ISSUER/DEFER_TASKRUN with EINVAL.
    common::refuse_io_uring(libc::EINVAL);
    assert_eq!(elide_transport_driver_new(common::workload(), 2, 8), 0);
    assert_eq!(elide_transport_last_error(), -(1000 + libc::EINVAL));
    let owner = elide_transport_owner_new(256);
    let output = elide_transport_buffer_new(owner, 256);
    let driver = elide_transport_driver_new(owner, 0, 8);
    assert_ne!(driver, 0, "AUTO must fall back to polling");
    assert_eq!(elide_transport_driver_backend(driver), 1);
    let length = elide_transport_driver_fallback(driver, output);
    let mut view = BufferView::default();
    // SAFETY: output is retained and view is writable for the complete BufferView.
    assert_eq!(unsafe { elide_transport_buffer_view(output, &mut view) }, 0);
    // SAFETY: fallback initialized length bytes in output, which remains retained through this read.
    let reason = unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), length as usize) };
    let reason = std::str::from_utf8(reason).unwrap();
    assert!(reason.ends_with("(os error 22)"), "{reason}");
    assert_eq!(elide_transport_driver_release(driver), 0);
    assert_eq!(elide_transport_buffer_release(output), 0);
    assert_eq!(elide_transport_owner_release(owner), 0);
  })
  .join()
  .unwrap();
}

#[test]
fn foreign_threads_reach_handles_minted_in_another_domain() {
  let owner = elide_transport_owner_new(64);
  let buffer = elide_transport_buffer_new(owner, 16);
  assert_ne!(buffer, 0);
  let (remote, frozen) = std::thread::spawn(move || {
    let remote = elide_transport_buffer_new(owner, 16);
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(buffer, &mut view) }, 0);
    assert_eq!(view.capacity, 16);
    // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
    assert_eq!(unsafe { elide_transport_buffer_freeze(buffer, 4) }, 0);
    (remote, elide_transport_buffer_slice(buffer, 0, 2))
  })
  .join()
  .unwrap();
  assert_ne!(remote, 0);
  assert_ne!(frozen, 0);
  assert_eq!(elide_transport_owner_used(owner), 32);
  assert_eq!(elide_transport_buffer_release(remote), 0);
  assert_eq!(elide_transport_buffer_release(remote), INVALID);
  assert_eq!(elide_transport_buffer_release(buffer), 0);
  assert_eq!(elide_transport_buffer_release(frozen), 0);
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
fn owner_release_stops_allocation_through_a_memoized_budget() {
  let owner = elide_transport_owner_new(64);
  assert_ne!(elide_transport_buffer_new(owner, 16), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
  assert_eq!(elide_transport_buffer_new(owner, 16), 0);
}

fn endpoint(owner: u64, address: std::net::SocketAddr) -> u64 {
  let std::net::SocketAddr::V4(v4) = address else {
    panic!("ipv4 endpoint expected")
  };
  let handle = elide_transport_buffer_new(owner, 24);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  let mut bytes = [0u8; 24];
  bytes[..4].copy_from_slice(&v4.ip().octets());
  bytes[16..18].copy_from_slice(&address.port().to_ne_bytes());
  bytes[18..20].copy_from_slice(&4u16.to_ne_bytes());
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.address.cast::<u8>(), 24) };
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(handle, 24) }, 0);
  handle
}

fn completion(driver: u64, batch: u64, kind: u32) -> (u64, i64) {
  for _ in 0..64 {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, 1_000_000_000, batch, 8) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    for index in 0..count as usize {
      // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
      let event = unsafe { &*view.address.cast::<NativeEvent>().add(index) };
      if event.kind == kind {
        return (event.value, event.result);
      }
    }
  }
  panic!("no completion of kind {kind}");
}

fn payload(handle: u64) -> (*const u8, u64) {
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  (view.address.cast::<u8>(), view.length)
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn pooled_receive_storage_is_reused_without_reviving_its_handle() {
  use std::io::Write;

  let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let owner = elide_transport_owner_new(1024 * 1024);
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 8);
  let endpoint = endpoint(owner, listener.local_addr().unwrap());
  let socket = elide_transport_socket_connect(common::workload(), driver, endpoint);
  assert_ne!(socket, 0);
  let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
  // io_uring submits the connect only once the proactor is polled, so accept cannot block first.
  let accepted = std::thread::spawn(move || listener.accept().unwrap().0);
  assert_eq!(completion(driver, batch, 1).1, 0);
  let mut peer = accepted.join().unwrap();

  peer.write_all(b"hello").unwrap();
  assert_ne!(
    elide_transport_socket_receive_new(common::workload(), driver, socket, owner, 4096),
    0
  );
  let (first, received) = completion(driver, batch, 3);
  assert!(received > 0);
  let (address, length) = payload(first);
  assert_eq!(length, received as u64);
  assert_eq!(elide_transport_buffer_release(first), 0);

  peer.write_all(b"world").unwrap();
  assert_ne!(
    elide_transport_socket_receive_new(common::workload(), driver, socket, owner, 4096),
    0
  );
  let (second, received) = completion(driver, batch, 3);
  assert!(received > 0);
  assert_ne!(second, first);
  assert_eq!(payload(second).0, address, "pooled storage was not reused");
  assert_eq!(elide_transport_buffer_release(first), INVALID);

  assert_eq!(elide_transport_buffer_release(second), 0);
  assert_eq!(elide_transport_socket_close(driver, socket), 0);
  assert_eq!(elide_transport_buffer_release(batch), 0);
  assert_eq!(elide_transport_buffer_release(endpoint), 0);
  assert_eq!(elide_transport_driver_release(driver), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "opens sockets and a driver")]
fn callback_batch_retirement_reclaims_an_unconsumed_receive() {
  use std::io::Write;
  struct State {
    driver: u64,
    value: u64,
    statuses: Vec<i32>,
  }
  unsafe extern "C" fn retire(context: u64, events: *const NativeEvent, count: u32) -> i32 {
    // SAFETY: The polling caller supplied this live, exclusively accessed stack context for the callback.
    let state = unsafe { &mut *(context as *mut State) };
    assert_eq!(count, 1);
    // SAFETY: The driver supplies a live initialized event for this synchronous callback.
    let event = unsafe { &*events };
    assert_eq!(event.kind, 3);
    state.value = event.value;
    state
      .statuses
      .push(elide_transport_socket_close(state.driver, event.socket));
    state.statuses.push(elide_transport_driver_release(state.driver));
    0
  }
  let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let owner = elide_transport_owner_new(1024 * 1024);
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 8);
  let address = endpoint(owner, listener.local_addr().unwrap());
  let socket = elide_transport_socket_connect(common::workload(), driver, address);
  let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
  let accepted = std::thread::spawn(move || listener.accept().unwrap().0);
  assert_eq!(completion(driver, batch, 1).1, 0);
  let mut peer = accepted.join().unwrap();
  peer.write_all(b"hello").unwrap();
  assert_ne!(
    elide_transport_socket_receive_new(common::workload(), driver, socket, owner, 4096),
    0
  );
  let mut state = State {
    driver,
    value: 0,
    statuses: Vec::new(),
  };
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while state.value == 0 && std::time::Instant::now() < deadline {
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    let result = unsafe {
      elide_transport_driver_poll_batch_callback(
        common::workload(),
        driver,
        50_000_000,
        8,
        Some(retire),
        &mut state as *mut State as u64,
      )
    };
    assert_eq!(result, if state.value == 0 { 0 } else { INVALID });
  }
  assert_ne!(state.value, 0);
  assert_eq!(state.statuses, [0, 0]);
  assert_eq!(elide_transport_buffer_release(state.value), INVALID);
  assert_eq!(elide_transport_buffer_release(address), 0);
  assert_eq!(elide_transport_buffer_release(batch), 0);
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}
