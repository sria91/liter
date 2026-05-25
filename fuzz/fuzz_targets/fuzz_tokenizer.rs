#![no_main]

use libfuzzer_sys::fuzz_target;
use sqlite3_tokenizer::tokenize;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let iter = tokenize(s);
        for res in iter {
            // Just ensure it doesn't panic
            let _ = res;
        }
    }
});
