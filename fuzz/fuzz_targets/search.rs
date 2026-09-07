#![no_main]
#[path = "../../tests/support/parser_oracles.rs"]
mod oracles;
libfuzzer_sys::fuzz_target!(|bytes: &[u8]| oracles::search(bytes));
