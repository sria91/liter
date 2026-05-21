#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Parser returns NotImplemented for now; exercise the tokenizer path.
        let _ = sqlite3_parser::parse_stmt(s);
    }
});
