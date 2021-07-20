pub mod action;
pub mod error;
pub mod event;
pub mod identity;
pub mod message;
pub mod peer;
mod ser;
pub mod topic;
mod util;

use crate::{
    action::Action,
    error::{Error, Result},
    event::Event,
    identity::{Pubkey as IdentityPubkey},
    message::{MessageSealed, MessageWrapped},
    peer::{Keypair, Pubkey as PeerPubkey, Peer},
    topic::Topic,
};
use getset::{Getters, Setters};
use serde_derive::{Serialize, Deserialize};
use std::{
    net::SocketAddr,
    ops::Deref,
};

/// Mothods for how this peer handles communication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mode {
    /// Agent mode only passes messages directly to trusted/established peers
    /// and discards any messages that aren't intended for the identities the
    /// peer directly manages. This mode will be the most common for most
    /// devices.
    ///
    /// Agent mode implies that this peer is mostly user-driven, ie, not fully
    /// automated (like some other modes).
    Agent,
    /// Relay mode sets this peer up to forward messages. Peers will register
    /// their interest to particular identities (w/ proofs) and other peers can
    /// publish messages for those identities to the relay, which will forward
    /// them to all interested parties.
    ///
    /// This is effectively a pubsub system. Messages that peers missed will
    /// have to be queried again (by broadcasting a query to the relay).
    ///
    /// Relay mode has an automated peer approval system: if you can prove you
    /// own an identity that it is set up to manage, it will peer with you.
    Relay,
    /// A storage node will save *syncing* messages it receives
    /// A storage node not only allows for relaying, but will
    /// store messages for some amount of time as well as full profile data, which
    /// allows peers (who can prove their identity) to fully sync new devices
    /// without having one of their other devices online.
    StorageNode,
}

/// Holds information about a connection to a peer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Getters)]
#[getset(get = "pub")]
pub struct ConnectionInfo {
    inner: SocketAddr,
}

impl ConnectionInfo {
    /// Create a new connection info object
    pub fn new(addr: &str) -> Result<Self> {
        Ok(Self {
            inner: addr.parse()?
        })
    }
}

impl Deref for ConnectionInfo {
    type Target = SocketAddr;

    fn deref(&self) -> &Self::Target {
        self.inner()
    }
}

impl From<SocketAddr> for ConnectionInfo {
    fn from(sock: SocketAddr) -> Self {
        Self {
            inner: sock,
        }
    }
}

/// Holds information about a peer.
#[derive(Debug, Getters, Setters)]
#[getset(get = "pub", get_mut, set)]
pub struct PeerInfo {
    /// The peer themselves
    peer: Peer,
    /// The connection information associated with this peer
    connection_info: ConnectionInfo,
    /// Whether peering is active (ie, we've sent a `PeerConfirm` event)
    active: bool,
}

impl PeerInfo {
    /// Create a new PeerInfo
    pub fn new(peer: Peer, connection_info: ConnectionInfo, active: bool) -> Self {
        Self {
            peer,
            connection_info,
            active,
        }
    }
}

/// The main interface to the syncing system: this produces events for both
/// communicating to other peers and interacting with the database.
#[derive(Debug, Getters)]
#[getset(get = "pub")]
pub struct Sync {
    /// The mode of operation for this peer.
    mode: Mode,
    /// The name of our peer.
    name: String,
    /// Our comm secret keypair. Used to encrypt/decrypt messages to peers.
    keypair: Keypair,
    /// A list of peers we are in contact with. They do not necessarily need to
    /// be active.
    peers: Vec<PeerInfo>,
    /// A list of subscriptions that peers have.
    subscriptions: Vec<Topic>,
    /// Track outgoing peer contact initiations so we don't send PeerInit to any
    /// random dork who Hello's us.
    outgoing_peer_init: Vec<ConnectionInfo>,
    /// Identities this peer manages
    identities: Vec<IdentityPubkey>,
}

impl Sync {
    /// Find a peer in our peering list by pubkey.
    fn find_peer(&self, pubkey: &PeerPubkey, active: Option<bool>) -> Option<&PeerInfo> {
        self.peers.iter().find(|p| {
            let active_bool = match active {
                Some(active_val) => p.active() == &active_val,
                None => true,
            };
            active_bool && p.peer().pubkey() == pubkey
        })
    }

    /// Find an existing outgoing peer request.
    fn find_outgoing_peer_init(&self, connection_info: &ConnectionInfo) -> Option<&ConnectionInfo> {
        self.outgoing_peer_init.iter().find(|r| r == &connection_info)
    }

    /// Create a new Sync
    pub fn new(mode: Mode, name: String, keypair: Keypair) -> Self {
        Self {
            mode,
            name,
            keypair,
            peers: Vec::new(),
            subscriptions: Vec::new(),
            outgoing_peer_init: Vec::new(),
            identities: Vec::new(),
        }
    }

    /// Process an incoming message meant for the sync system, returning a set
    /// of "suggested" actions to take based on the result.
    pub fn process_incoming_message(&mut self, message_wrapped: &MessageWrapped, connection_info: ConnectionInfo) -> Result<Vec<Action>> {
        let message = match message_wrapped {
            // unwrap the sealed message and hand it back
            MessageWrapped::Sealed(ref sealed) => sealed,
            // a peer is initating unencrypted communication. send them back a
            // `Hello` so they can have our key, then return.
            MessageWrapped::Init(ref pubkey) => {
                let new_peer = Peer::tmp(pubkey, self.keypair().seckey());
                // if e haven't already, add this peer to our list (as inactive)
                if self.find_peer(pubkey, None).is_none() {
                    self.peers.push(PeerInfo::new(new_peer.clone(), connection_info.clone(), false));
                }
                let body = Event::Hello;
                let outgoing = MessageSealed::create(&self.keypair().to_pubkey(), &new_peer, util::now(), message::gen_nonce(), body, vec![])?;
                let outgoing_wrapped = MessageWrapped::Sealed(outgoing);
                return Ok(vec![
                    Action::MessageSend(connection_info, outgoing_wrapped),
                ]);
            }
        };
        let peer_from_maybe = self.find_peer(message.pubkey_sender(), Some(true));
        let message_opened = if let Some(peer_from) = peer_from_maybe {
            message.open(peer_from.peer())?
        } else {
            let tmp_peer = Peer::tmp(message.pubkey_sender(), self.keypair().seckey());
            message.open(&tmp_peer)?
        };
        let event = message_opened.body();

        // if we don't have a known peer for this event, only allow it to
        // process if it is one of a few allowable event types
        if peer_from_maybe.is_none() {
            match event {
                // Hello and PeerInit don't require a known sender
                Event::Hello | Event::PeerInit {..} => { }
                // every other event does require a known sender
                _ => Err(Error::PeerNotFound(message.pubkey_sender().clone()))?,
            }
        };

        // ok, now that we got all that out of the way...
        match event {
            Event::Hello => {
                // don't process a `Helloooo` unless we initiated it.
                if self.find_outgoing_peer_init(&connection_info).is_none() {
                    return Ok(vec![]);
                }
                let pubkey = message.pubkey_sender();
                let new_peer = Peer::tmp(pubkey, self.keypair().seckey());
                // if we haven't already, add this peer to our list (as inactive)
                if self.find_peer(pubkey, None).is_none() {
                    self.peers.push(PeerInfo::new(new_peer.clone(), connection_info.clone(), false));
                }
                // now that the peer is in our stupid list, we can remove the
                // outgoing_peer_init entry
                self.outgoing_peer_init.retain(|c| {
                    !(c == &connection_info)
                });

                let topics = vec![];
                let body = Event::PeerInit { mode: self.mode().clone(), name: self.name().clone(), topics, };
                let outgoing = MessageSealed::create(&self.keypair().to_pubkey(), &new_peer, util::now(), message::gen_nonce(), body, vec![])?;
                let outgoing_wrapped = MessageWrapped::Sealed(outgoing);
                return Ok(vec![
                    Action::MessageSend(connection_info, outgoing_wrapped),
                ]);
            }
            _ => panic!("LOL"),
        }

        Ok(vec![])
    }

    /// Initialize outgoing "relations"  ;) ;) with another peer ;)
    pub fn peer_init(&mut self, addr: &str) -> Result<Vec<Action>> {
        let connection_info = ConnectionInfo::new(addr)?;
        let message_wrapped = MessageWrapped::Init(self.keypair.to_pubkey().clone());
        let existing_req = self.find_outgoing_peer_init(&connection_info);
        if existing_req.is_none() {
            self.outgoing_peer_init.push(connection_info.clone());
        }
        Ok(vec![Action::MessageSend(connection_info, message_wrapped)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}

