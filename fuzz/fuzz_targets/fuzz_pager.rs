#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Feed arbitrary bytes to the record decoder — should never panic.
    let _ = sqlite3_record::decode_record(data);
});
