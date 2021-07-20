//! Events are what we respond to when syncing with other tp2p clients. They
//! are always incoming. "Events" that we create as a result are [Actions](crate::action::Action)
//! and can create messages to other peers or modify database values.

use crate::{
    Mode,
    message::MessageID,
    topic::{Subscription, Topic},
};
use serde_derive::{Serialize, Deserialize};
use sodiumoxide::crypto::hash::sha256;

/// Represents a p2p event we should (probably) respond to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Used to initiate encrypted communication.
    Hello,
    /// Used when a new peer wants to establish a connection with our group.
    PeerInit {
        /// The peering mode the initiating peer uses.
        mode: Mode,
        /// The name the initiating peer wishes to use. Hopefully not an 18GB
        /// string.
        name: String,
        /// Topics the initating peer is willing to publish to.
        topics: Vec<Topic>,
    },
    /// A confirmation that peering is approved.
    PeerConfirm {
        /// The peering mode the confirming peer uses.
        mode: Mode,
        /// The peer name of the confirming peer.
        name: String,
        /// Topics the confirming peer is willing to publish to.
        topics: Vec<Topic>,
    },
    /// Ask peers for messages with the given IDs. No need for a specific
    /// response type because the peer will just send the messages as the result
    QueryMessagesById {
        /// The IDs of the messages we want
        ids: Vec<MessageID>,
    },
    /// Ask peers for messages after a certain depth. No need for a specific
    /// response type because the peer will just send the messages as the result
    QueryMessagesByDepth {
        /// The topic we're querying messages for
        topic: Topic,
        /// Find messages after this depth
        depth: u64,
    },
    /// See which peers have the given file chunks
    QueryPeerFileChunks(Vec<sha256::Digest>),
    /// Here are the chunks we have.
    ResultQueryPeerFileChunks(Vec<sha256::Digest>),
    /// Advertise that this peer provides these new topics. This list does not
    /// need to be exhaustive, it can simply add to existing lists.
    TopicsAdded(Vec<Topic>),
    /// Advertise that this peer no longer provides the given topics.
    TopicsRemoved(Vec<Topic>),
    /// Subscribe to a topic.
    Subscribe(Vec<Subscription>),
    /// Unsubscribe from a topic
    Unsubscribe(Vec<Subscription>),
    /// Sync a change to the data profile
    Sync(()),
    /// Ask for file chunks from a peer.
    ///
    /// Generally, we use `QueryPeerFileChunks` first to see who has what chunks
    /// and then we'll find one peer that has the chunks we need and ask them to
    /// throw some chunks our way.
    GetFileChunks(Vec<sha256::Digest>),
    /// A message carrying a chunk of a file.
    FileChunk(sha256::Digest, Vec<u8>),
}

