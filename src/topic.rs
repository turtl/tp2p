use crate::{
    identity::{IdentityProof, Pubkey as IdentityPubkey},
};
use serde_derive::{Serialize, Deserialize};

/// Represents a p2p event we should (probably) respond to.

/// Describes a topic that can be subscribed to. Peers will subscribe to topics
/// in order to be notified of messages that are important to them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Topic {
    /// Subscribe to any messages destined for a particular identity.
    Identity(IdentityPubkey),
}

/// A subscription to a topic. Generally, this requires some kind of proof or
/// authorization that determines the validity of the subscription.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Subscription {
    /// A subscription to any messages pertaining to an identity. Requires proof
    /// that we are owners of this identity (verifiable via the public key).
    Identity(IdentityPubkey, IdentityProof),
}

