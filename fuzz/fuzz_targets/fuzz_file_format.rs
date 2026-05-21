#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Placeholder: feed arbitrary .db file bytes to the pager/btree once
    // those layers are implemented.
    let _ = data;
});
