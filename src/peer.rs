use sodiumoxide;

pub enum Protocol {
}

/// Holds the information necessary to reach a peer
pub struct PeerSpec {
    pub protocol: Protocol,
    pub address: String,
    pub port: u32,
}

impl PeerSpec {
    /// Create a new PeerSpec
    pub fn new(protocol: Protocol, address: String, port: u32) -> Self {
        Self {
            protocol,
            address,
            port,
        }
    }
}

/// A peer's unique ID
pub struct PeerID {
    pub client_id: Vec<u8>,
}

/// Holds information about a peer.
pub struct Peer {
    pub id: PeerID,
    pub pubkey: sodiumoxide::crypto::box_::curve25519xsalsa20poly1305::PublicKey,
    pub spec: PeerSpec,
}
