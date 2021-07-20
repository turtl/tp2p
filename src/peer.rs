use crate::{
    Mode,
    error::{Error, Result},
};
use getset::{Getters, Setters};
use serde_derive::{Serialize, Deserialize};
use sodiumoxide::{
    crypto::box_::curve25519xsalsa20poly1305,
};
use std::ops::Deref;

/// A peer's public key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct Pubkey {
    inner: curve25519xsalsa20poly1305::PublicKey,
}

impl Deref for Pubkey {
    type Target = curve25519xsalsa20poly1305::PublicKey;
    fn deref(&self) -> &Self::Target {
        self.inner()
    }
}

impl Pubkey {
    /// Create a new public key.
    pub fn new(pubkey: curve25519xsalsa20poly1305::PublicKey) -> Self {
        Self {
            inner: pubkey,
        }
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
#[derive(Debug, Clone, PartialEq, Getters, Setters)]
pub struct Peer {
    /// The operating modes this peer advertised when peering was set up.
    #[getset(get = "pub", set = "pub(crate)")]
    mode: Mode,
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
    pub(crate) fn new(mode: Mode, name: String, pubkey: Pubkey, our_seckey: &curve25519xsalsa20poly1305::SecretKey) -> Self {
        let precomputed = curve25519xsalsa20poly1305::precompute(&pubkey, &our_seckey);
        Self {
            mode,
            name,
            pubkey,
            precomputed,
        }
    }

    /// A convenience function to create a temporary, nameless, modeless peer.
    pub(crate) fn tmp(pubkey: &Pubkey, our_seckey: &curve25519xsalsa20poly1305::SecretKey) -> Self {
        Self::new(Mode::Agent, "<tmp>".into(), pubkey.clone(), our_seckey)
    }
}

