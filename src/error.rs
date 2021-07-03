//! The main error enum for the project lives here, and documents the various
//! conditions that can arise while interacting with the system.

use thiserror::Error;

/// This is our error enum. It contains an entry for any part of the system in
/// which an expectation is not met or a problem occurs.
#[derive(Error, Debug)]
pub enum Error {
    /// An error while engaging in deserialization.
    #[error("deserialization error")]
    Deserialize(#[from] rmp_serde::decode::Error),

    /// An IO/net error
    #[error("io error {0:?}")]
    IoError(#[from] std::io::Error),

    /// Failed to open a sealed message. This is a bummer, man.
    #[error("failed to open a sealed message")]
    MessageOpenFailed,

    /// An error while engaging in serialization.
    #[error("serialization error")]
    Serialize(#[from] rmp_serde::encode::Error),
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        // i'm sorry...
        //
        // TODO: implement a real PartialEq. cannot derive because
        // std::io::Error et al are not eq-able. tonight we dine in hell.
        format!("{:?}", self) == format!("{:?}", other)
    }
}

/// Wraps `std::result::Result` around our `Error` enum
pub type Result<T> = std::result::Result<T, Error>;

