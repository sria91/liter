#![no_main]

use libfuzzer_sys::fuzz_target;
use sqlite3::Connection;

fuzz_target!(|data: &[u8]| {
    if let Ok(sql) = std::str::from_utf8(data) {
        if let Ok(conn) = Connection::open_in_memory() {
            // Execute arbitrary SQL strings and ensure the engine catches all errors
            // gracefully instead of panicking or OOMing.
            let _ = conn.execute(sql, [] as [(); 0]);
        }
    }
});
