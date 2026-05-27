//! SSR error types.

use std::fmt;

/// Single-variant error for SSR operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsrError(pub(crate) String);

impl fmt::Display for SsrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SSR error: {}", self.0)
    }
}

impl std::error::Error for SsrError {}

/// Bail macro for early-return with SsrError.
macro_rules! bail {
    ($e:expr) => { return Err($crate::SsrError($e.to_string())) };
    ($fmt:expr, $($arg:tt)*) => { return Err($crate::SsrError(format!($fmt, $($arg)*))) };
}

#[allow(unused_imports)]
pub(crate) use bail;
