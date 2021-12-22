//! Events are what we respond to when syncing with other tp2p clients. They
//! are always incoming. "Events" that we create as a result are [Actions](crate::action::Action)
//! and can create messages to other peers or modify database values.

use crate::{
    message::MessageID,
};
use serde_derive::{Serialize, Deserialize};

/// Represents a p2p event. All the events here allow setting up topic-based
/// communication channels between peers, except for the `User` event which is
/// used to send your actual application messages between peers.
///
/// In other words, most of the garbage here is used by the tp2p system. The
/// `User` event is for you!
///
/// There are three generics: `U` ("User") which is your app's custom syncing
/// type, `T` ("Topic") which describes topics, and `S` ("Subscription") which
/// describes a subscription (and implements the [Subscription](crate::subscription::Subscription)
/// trait). Subscriptions are generally a wrapper around a topic type (`T`,
/// here) that also contains some verifiable cryptographic proof that the agent
/// posting the subscription has access to the given topic. The details of how
/// this is accomplished is entirely up to you, dear friend, via the
/// `Subscription` trait.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event<U, T, S> {
    /// Used to initiate encrypted communication.
    Hello,
    /// Used when a new peer wants to establish a connection with our group.
    PeerInit {
        /// The name the initiating peer wishes to use. Hopefully not an 18GB
        /// string.
        name: String,
    },
    /// A confirmation that peering is approved.
    PeerConfirm {
        /// The peer name of the confirming peer.
        name: String,
    },
    /// Shake, are you there?
    Ping,
    /// Yes, I'm right beside you.
    Pong,
    /// Ask peers for messages with the given IDs. No need for a specific
    /// response type because the peer will just send the messages as the result
    QueryMessagesByID {
        /// The IDs of the messages we want
        ids: Vec<MessageID>,
    },
    /// Ask peers for messages after a certain depth. No need for a specific
    /// response type because the peer will just send the messages as the result
    QueryMessagesByDepth {
        /// The topic we're querying messages for
        topic: T,
        /// Find messages after this depth
        depth: u64,
    },
    /// Subscribe to topic(s).
    Subscribe(Vec<S>),
    /// Unsubscribe from topic(s).
    Unsubscribe(Vec<T>),
    /// User-defined messages/events.
    ///
    /// This is where you'll send actual data between your peers.
    User(U),
}

