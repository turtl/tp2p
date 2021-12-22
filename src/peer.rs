use chrono::{DateTime, Utc};
use crate::{
    error::{Error, Result},
    sync::ConnectionInfo,
    topic::Topic,
};
use getset::{Getters, MutGetters, Setters};
use serde_derive::{Serialize, Deserialize};
use sodiumoxide::{
    crypto::box_::curve25519xsalsa20poly1305,
};
use std::{fmt, ops::Deref};

/// A peer's public key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct Pubkey {
    inner: curve25519xsalsa20poly1305::PublicKey,
}

impl Pubkey {
    /// Create a new public key.
    pub fn new(pubkey: curve25519xsalsa20poly1305::PublicKey) -> Self {
        Self {
            inner: pubkey,
        }
    }
}

impl Deref for Pubkey {
    type Target = curve25519xsalsa20poly1305::PublicKey;
    fn deref(&self) -> &Self::Target {
        self.inner()
    }
}

/// Convert bytes to base64
pub fn base64_encode<T: AsRef<[u8]>>(bytes: T) -> String {
    base64::encode_config(bytes.as_ref(), base64::URL_SAFE_NO_PAD)
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "pubkey::{}", base64_encode(self.deref()))
    }
}

/// A full keypair. Generally only used for THIS node, ie, not to be stored with
/// peers, because we shouldn't ever know their secret keys!
#[derive(Clone, Debug, Getters)]
#[getset(get = "pub")]
pub struct Keypair {
    pubkey: curve25519xsalsa20poly1305::PublicKey,
    seckey: curve25519xsalsa20poly1305::SecretKey,
}

impl Keypair {
    /// Create a new keypair from a raw public/secret keypair
    pub fn new(pubkey: curve25519xsalsa20poly1305::PublicKey, seckey: curve25519xsalsa20poly1305::SecretKey) -> Self {
        Self {
            pubkey,
            seckey,
        }
    }

    /// Create a random keypair.
    pub fn new_random() -> Self {
        let (pubkey, seckey) = curve25519xsalsa20poly1305::gen_keypair();
        Self::new(pubkey, seckey)
    }

    /// Create a new keypair from a seed.
    pub fn new_with_seed(seed_bytes: &[u8]) -> Result<Self> {
        let seed = curve25519xsalsa20poly1305::Seed::from_slice(seed_bytes)
            .ok_or(Error::KeypairFromSeedFailed)?;
        let (pubkey, seckey) = curve25519xsalsa20poly1305::keypair_from_seed(&seed);
        Ok(Self::new(pubkey, seckey))
    }

    /// Create a Pubkey from this Keypair.
    pub fn to_pubkey(&self) -> Pubkey {
        Pubkey::new(self.pubkey.clone())
    }
}

/// Holds information about a peer.
#[derive(Debug, Clone, PartialEq, Getters, Setters, Serialize, Deserialize)]
pub struct Peer {
    /// The name of our peer, probably a device name LOL
    #[getset(get = "pub", set = "pub(crate)")]
    name: String,
    /// The public key of the peer.
    #[getset(get = "pub", set)]
    pubkey: Pubkey,
    /// A precomputed key based on our secret key and the peer's public key.
    #[getset(get = "pub(crate)", set)]
    precomputed: curve25519xsalsa20poly1305::PrecomputedKey,
}

impl Peer {
    /// Create a new Peer.
    pub(crate) fn new(name: String, pubkey: Pubkey, our_seckey: &curve25519xsalsa20poly1305::SecretKey) -> Self {
        let precomputed = curve25519xsalsa20poly1305::precompute(&pubkey, &our_seckey);
        Self {
            name,
            pubkey,
            precomputed,
        }
    }

    /// A convenience function to create a temporary, nameless peer.
    pub(crate) fn tmp(pubkey: &Pubkey, our_seckey: &curve25519xsalsa20poly1305::SecretKey) -> Self {
        Self::new("<tmp>".into(), pubkey.clone(), our_seckey)
    }
}

/// Holds information about a peer.
#[derive(Debug, Clone, Getters, MutGetters, Setters, Serialize, Deserialize)]
#[getset(get = "pub", get_mut = "pub(crate)", set = "pub(crate)")]
pub struct PeerInfo {
    /// The peer themselves
    peer: Peer,
    /// The connection information associated with this peer
    connection_info: ConnectionInfo,
    /// Whether peering is active (ie, we've sent a `PeerConfirm` event)
    active: bool,
    /// Last time this peer pinged or ponged us
    last_ping: DateTime<Utc>,
}

impl PeerInfo {
    /// Create a new PeerInfo
    pub fn new(peer: Peer, connection_info: ConnectionInfo, active: bool, last_ping: DateTime<Utc>) -> Self {
        Self {
            peer,
            connection_info,
            active,
            last_ping,
        }
    }
}

/// Determines if/how we allow incoming peers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfirmationMode {
    /// Deny all peering requests, except the given whitelist.
    ///
    /// Good for private headless peers where confirmation isn't even possible
    /// (due to there being no good UI to get confirmation from).
    Private {
        whitelist: Vec<Pubkey>,
    },
    /// Allow all whitelisted peers, deny all blacklisted, and confirm any
    /// unknown peers.
    ///
    /// Good for peers with a UI that allows confirmations. This is basically a
    /// good default for any app-based peer.
    Confirm {
        whitelist: Vec<Pubkey>,
        blacklist: Vec<Pubkey>,
    },
    /// Allow all peering requests from peers. Deny blacklisted. Whitelist
    /// specifically for non-agent based peers (ie, other trusted relay nodes).
    ///
    /// Good for public relay/storage nodes.
    PublicAgent {
        whitelist: Vec<Pubkey>,
        blacklist: Vec<Pubkey>,
    },
}

/// Determines what topics are published to who.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubscriptionConfig {
    /// Allow all topics to be published to all peers except those in this list.
    Blacklist(Vec<(Pubkey, Topic)>),
    /// Prohobit all topics to be published to any peers except those in this
    /// list.
    Whitelist(Vec<(Pubkey, Topic)>),
}

/// Our peer configuration. Determines how/if we accept other peers, as well as
/// our topic configuration.
#[derive(Debug, Clone, Getters, Serialize, Deserialize)]
#[getset(get = "pub(crate)")]
pub struct PeeringConfig {
    /// How we confirm other peers.
    confirmation: ConfirmationMode,
    /// Topic subscription config
    subscriptions: SubscriptionConfig,
}

impl PeeringConfig {
    /// Create a new peering config.
    pub fn new(confirmation: ConfirmationMode, subscriptions: SubscriptionConfig) -> Self {
        Self {
            confirmation,
            subscriptions,
        }
    }

    pub(crate) fn can_confirm(&self, peer_pubkey: &Pubkey) -> Option<bool> {
        match self.confirmation() {
            ConfirmationMode::Private { whitelist } => {
                if whitelist.iter().find(|x| x == &peer_pubkey).is_some() {
                    Some(true)
                } else {
                    Some(false)
                }
            }
            ConfirmationMode::Confirm { whitelist, blacklist } => {
                if blacklist.iter().find(|x| x == &peer_pubkey).is_some() {
                    Some(false)
                } else if whitelist.iter().find(|x| x == &peer_pubkey).is_some() {
                    Some(true)
                } else {
                    None
                }
            }
            ConfirmationMode::PublicAgent { whitelist, blacklist } => {
                if blacklist.iter().find(|x| x == &peer_pubkey).is_some() {
                    Some(false)
                } else if whitelist.iter().find(|x| x == &peer_pubkey).is_some() {
                    Some(true)
                } else {
                    Some(true)
                }
            }
        }
    }
}

