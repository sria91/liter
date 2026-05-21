#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|_data: &[u8]| {
    // Placeholder: once the VDBE is implemented, feed compiled programs here.
});
