//! Actions tell whoever is running our little syncing action what to do. For
//! instance, store an item in the local DB, send a message to a peer, share a
//! photo with your grandparents, launch a set of nukes, etc.

use crate::{
    event::Event,
    message::MessageWrapped,
    peer::{Pubkey as PeerPubkey, PeerInfo},
    sync::{ConnectionInfo, PendingPeer},
};
use serde_derive::{Serialize, Deserialize};

/// Represents an action the sync system wishes to perform on itself.
///
/// Basically, instead of mutating the [Sync](crate::Sync) object when messages
/// are processed, we hand back a list of changes that are recommended to be
/// changed in the sync object, which allows those changes to be persisted to
/// any storage medium we deem appropriate (or thrown out into the abyss like
/// the trash they are).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncAction<T> {
    /// Make sure a peer exists (if not, adds it to the end of the peers list,
    /// otherwise updates the existing peer).
    SetPeer(PeerInfo),
    /// Make sure a peer is absent (matched by pubkey)
    UnsetPeer(PeerPubkey),
    /// Add a peer to the pending peers list
    SetPendingPeer(PendingPeer),
    /// Remove a peer from the pending peers list
    UnsetPendingPeer(ConnectionInfo),
    /// Create a topic subscription
    SetSubscription(T, PeerInfo),
    /// Remove a subscription
    UnsetSubscription(T, PeerInfo),
}

/// Holds an individual action. This tells other parts of the system what to do
/// in relation to either storing or sending data.
#[derive(Clone, Debug)]
pub enum Action<U, T, S> {
    /// Send to grandparents.
    MessageSend(ConnectionInfo, MessageWrapped),
    /// Confirm something with the user (hopefully we are not in headless mode).
    Confirm(Event<U, T, S>),
    /// An action pertaining to a [Sync](crate::sync) object.
    Sync(SyncAction<T>),
}

