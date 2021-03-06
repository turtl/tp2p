#[cfg(test)]
use chrono::{DateTime, Utc};

/// A wrapper around Utc::now() that lets us mock if we need to for testing.
#[cfg(test)]
pub(crate) fn now() -> DateTime<Utc> {
    Utc::now()
}

