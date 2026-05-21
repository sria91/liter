#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;

#[derive(Debug, Arbitrary)]
struct KeyValuePair {
    key: Vec<u8>,
    value: Vec<u8>,
}

fuzz_target!(|_ops: Vec<KeyValuePair>| {
    // Placeholder: once the B-tree is implemented, feed key/value sequences here.
});
