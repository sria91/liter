#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Consume all tokens, ignoring errors.
        let _: Vec<_> = sqlite3_tokenizer::tokenize(s).collect();
    }
});
