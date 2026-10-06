#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| bemo_fuzz::abi(data));
