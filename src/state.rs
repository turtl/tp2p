use crate::{
    peer::{Peer, PeerSpec},
};

pub struct PeerState {
    peers: Vec<Peer>,
}

/// Start the state system
pub async fn start(peers: &Vec<PeerSpec>) {
}

