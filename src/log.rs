//! Minimal tagged logging to stderr.

/// Prints `[TAG] message` to stderr.
pub fn log(tag: &str, message: &str) {
    eprintln!("[{tag}] {message}");
}

#[macro_export]
macro_rules! log_warn {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log::log($tag, &format!($($arg)*))
    };
}
pub use crate::log_warn;
