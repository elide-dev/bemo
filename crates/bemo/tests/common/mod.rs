use bemo::driver::Backend;

pub fn backend() -> Backend {
  match std::env::var("ELIDE_TRANSPORT_TEST_BACKEND")
    .as_deref()
    .unwrap_or("auto")
  {
    "auto" => Backend::Auto,
    "iocp" if cfg!(windows) => Backend::Iocp,
    "kqueue" if cfg!(target_os = "macos") => Backend::Polling,
    "epoll" if cfg!(target_os = "linux") => Backend::Polling,
    "io-uring" if cfg!(target_os = "linux") => Backend::IoUring,
    name => panic!("unsupported test backend: {name}"),
  }
}

/// Refuse `io_uring_setup` with `errno` on the calling thread only, as a sandbox's seccomp profile
/// does. The filter dies with the thread, so run each case on its own spawned thread.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn refuse_io_uring(errno: i32) {
  const ARCH: u32 = if cfg!(target_arch = "x86_64") {
    0xC000_003E
  } else {
    0xC000_00B7
  };
  let load = |offset: u32| libc::sock_filter {
    code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
    jt: 0,
    jf: 0,
    k: offset,
  };
  let equal = |value: u32, skip: u8| libc::sock_filter {
    code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
    jt: 0,
    jf: skip,
    k: value,
  };
  let ret = |value: u32| libc::sock_filter {
    code: (libc::BPF_RET | libc::BPF_K) as u16,
    jt: 0,
    jf: 0,
    k: value,
  };
  // seccomp_data: syscall number at offset 0, audit architecture at offset 4.
  let mut program = [
    load(4),
    equal(ARCH, 3),
    load(0),
    equal(libc::SYS_io_uring_setup as u32, 1),
    ret(libc::SECCOMP_RET_ERRNO | errno as u32),
    ret(libc::SECCOMP_RET_ALLOW),
  ];
  let filter = libc::sock_fprog {
    len: program.len() as u16,
    filter: program.as_mut_ptr(),
  };
  let one: libc::c_ulong = 1;
  let zero: libc::c_ulong = 0;
  let mode = libc::SECCOMP_MODE_FILTER as libc::c_ulong;
  assert_eq!(
    // SAFETY: This prctl option takes integer arguments, passed with the required variadic width.
    unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, one, zero, zero, zero) },
    0,
    "{}",
    std::io::Error::last_os_error()
  );
  assert_eq!(
    // SAFETY: filter and its initialized instruction array remain live while the kernel copies them.
    unsafe { libc::prctl(libc::PR_SET_SECCOMP, mode, &raw const filter) },
    0,
    "{}",
    std::io::Error::last_os_error()
  );
}

/// Process-wide workload for tests that exercise a single workload.
#[allow(dead_code)]
pub fn workload() -> u64 {
  static WORKLOAD: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
  *WORKLOAD.get_or_init(|| bemo::abi::elide_transport_owner_new(1))
}
