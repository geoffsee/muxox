use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

/// Initialise the debug log. Call once at startup.
/// Writes to `muxox-debug.log` in the current directory.
pub fn init() {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("muxox-debug.log")
        .expect("failed to open muxox-debug.log");
    LOG_FILE.set(Mutex::new(file)).ok();
    START.set(Instant::now()).ok();
    debug("logger initialised");
}

/// Write a timestamped line to the debug log.
pub fn debug(msg: &str) {
    if let Some(f) = LOG_FILE.get() {
        let elapsed = START.get().map(|s| s.elapsed()).unwrap_or_default();
        if let Ok(mut f) = f.lock() {
            let _ = writeln!(f, "[{:.3}s] {}", elapsed.as_secs_f64(), msg);
            let _ = f.flush();
        }
    }
}
