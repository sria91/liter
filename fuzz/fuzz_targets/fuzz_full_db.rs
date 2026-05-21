#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(sql) = std::str::from_utf8(data) {
        // Run arbitrary SQL through the full database engine. Should never panic.
        let conn = sqlite3::Connection::open_in_memory().unwrap();
        let _ = conn.execute(sql, [] as [(); 0]);
    }
});
