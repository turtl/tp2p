//! The main error enum for the project lives here, and documents the various
//! conditions that can arise while interacting with the system.

use crate::{
    peer::{Pubkey as PeerPubkey},
};
use thiserror::Error;

/// This is our error enum. It contains an entry for any part of the system in
/// which an expectation is not met or a problem occurs.
#[derive(Error, Debug)]
pub enum Error {
    /// An error while engaging in deserialization.
    #[error("deserialization error")]
    Deserialize(#[from] rmp_serde::decode::Error),

    /// The wrong function was called when processing an event
    #[error("incorrect event processing function called")]
    EventMismatch,

    /// Failed to create a keypair from seed
    #[error("failed to create a keypair from seed")]
    KeypairFromSeedFailed,

    /// A `MessageWrapped` was given that was not the `Init` variant
    #[error("given wrapped message was not an `Init`")]
    MessageNotInit,

    /// Failed to open a sealed message. This is a bummer, man.
    #[error("failed to open a sealed message")]
    MessageOpenFailed,

    /// A peer public key was not provided while processing an event that
    /// requires it to be present.
    #[error("missing peer pubkey")]
    MissingPubkey,

    /// Cannot parse an address
    #[error("error parsing address {0:?}")]
    ParseError(#[from] std::net::AddrParseError),

    /// A message was received from a peer we haven't linked with yet.
    #[error("peer {0:?} not found")]
    PeerNotFound(PeerPubkey),

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

