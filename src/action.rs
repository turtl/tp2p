//! Actions tell whoever is running our little syncing action what to do. For
//! instance, store an item in the local DB, send a message to a peer, share a
//! photo with your grandparents, launch a set of nukes, etc.

use crate::{
    ConnectionInfo,
    message::MessageWrapped,
};

/// Holds an individual action. This tells other parts of the system what to do
/// in relation to either storing or sending data.
#[derive(Clone, Debug)]
pub enum Action {
    /// Send to grandparents.
    MessageSend(ConnectionInfo, MessageWrapped),
}

