mod common;

use bemo::driver::Driver;

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn closed_drivers_allow_repeated_server_creation() {
  for generation in 0..30 {
    let drivers: Vec<_> = (0..4)
      .map(|_| {
        Driver::new(common::backend(), 1024).unwrap_or_else(|error| panic!("driver generation {generation}: {error}"))
      })
      .collect();
    drop(drivers);
  }
}
