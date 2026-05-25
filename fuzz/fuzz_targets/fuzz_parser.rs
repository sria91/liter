#![no_main]

use libfuzzer_sys::fuzz_target;
use sqlite3_parser::Parser;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut parser = Parser::new(s);
        let _ = parser.parse_all();
    }
});
