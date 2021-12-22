use chrono::{DateTime, Utc};
use crate::{
    event::Event,
    error::{Error, Result},
    peer::{Peer, Pubkey as PeerPubkey},
    ser,
};
use getset::{Getters};
use serde_derive::{Serialize, Deserialize};
use sodiumoxide::{
    crypto::box_::curve25519xsalsa20poly1305::{self, Nonce},
};

/// Create a new random message nonce. This also gives us a place to inject mock
/// nonce values for testing.
pub(crate) fn gen_nonce() -> Nonce {
    curve25519xsalsa20poly1305::gen_nonce()
}

/// A message ID. Allows us to distinguish messages.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct MessageID {
    /// Timestamp of the ID
    timestamp: DateTime<Utc>,
    /// Random nonce for each message lol
    nonce: Nonce,
}

impl MessageID {
    /// Create a new MessageID
    pub fn new(timestamp: DateTime<Utc>, nonce: Nonce) -> Self {
        Self {
            timestamp,
            nonce,
        }
    }
}

/// Links this message as a child of one or more other messages *in the context
/// of a single topic*. This also 
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct TopicGraphEntry<T> {
    /// The topic this entry pertains to.
    topic: T,
    /// Holds the MessageIDs of any parent messages (generally one, but can be
    /// multiple). This helps with partial ordering of events.
    parents: Vec<MessageID>,
    /// Effectively gives us a loose ordering of events. This message's depth
    /// value should always be greater than the messages it is referencing.
    depth: u64,
}

impl<T> TopicGraphEntry<T> {
    /// Create a new TopicGraphEntry, allowing a message to link back to previous
    /// messages in a particular topic, allowing for ordering of events.
    pub fn new(topic: T, parents: Vec<MessageID>, depth: u64) -> Self {
        Self {
            topic,
            parents,
            depth,
        }
    }
}

/// A message, the basis for communication between peers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct Message<U, T, S> {
    /// The id of this message. Hopefully unique.
    ///
    /// The receiver should store the message IDs of all messages it has
    /// responded to within the time range T and ignore any repeats. This
    /// eliminates duplicates/replays.
    id: MessageID,
    /// Allows (partial) ordering of messages on a topic-based basis. If left
    /// empty signals that the message is a broadcast (which is the case for
    /// most peering messages since ordering isn't really necessary).
    topic_graph: Vec<TopicGraphEntry<T>>,
    /// "This is the actual message body," he said with a boyish grin.
    body: Event<U, T, S>,
}

impl<U, T, S> Message<U, T, S> {
    /// Create a new message
    pub fn new(id: MessageID, body: Event<U, T, S>, topic_graph: Vec<TopicGraphEntry<T>>) -> Self {
        Self {
            id,
            topic_graph,
            body,
        }
    }
}

/// A sealed message. Can only be opened using the private key matching the
/// public key it was encrypted with. Also, the contents must match the
/// signature of the private key that signed it.
#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct MessageSealed {
    /// Random nonce for each message lol
    nonce: Nonce,
    /// Who this message is supposedly from (we'll know for certain once we
    /// attempt to decrypt).
    pubkey_sender: PeerPubkey,
    /// The encryped body of our message. This deserializes to a
    /// [Message](crate::message::Message).
    sealed_body: Vec<u8>,
}

impl MessageSealed {
    /// Creates a new signed and encrypted message meant for a single recipient.
    pub fn create<U, T, S>(pubkey_sender: &PeerPubkey, peer_to: &Peer, now: DateTime<Utc>, nonce: Nonce, body: Event<U, T, S>, topic_graph: Vec<TopicGraphEntry<T>>) -> Result<Self>
        where U: serde::Serialize,
              T: serde::Serialize,
              S: serde::Serialize,
    {
        let id = MessageID::new(now, nonce);
        let msg = Message::new(id, body, topic_graph);
        Self::seal(pubkey_sender, peer_to, &msg)
    }

    /// Seal an existing message.
    ///
    /// Or don't. I don't care.
    pub fn seal<U, T, S>(pubkey_sender: &PeerPubkey, peer_to: &Peer, message: &Message<U, T, S>) -> Result<Self>
        where U: serde::Serialize,
              T: serde::Serialize,
              S: serde::Serialize,
    {
        let serialized_body = ser::serialize(&message)?;
        let sealed = curve25519xsalsa20poly1305::seal_precomputed(&serialized_body, message.id().nonce(), peer_to.precomputed());
        let sealed_message = Self {
            nonce: message.id().nonce().clone(),
            pubkey_sender: pubkey_sender.clone(),
            sealed_body: sealed,
        };
        Ok(sealed_message)
    }

    /// Open a sealed message and return the inner Message object.
    pub fn open<U, T, S>(&self, peer_from: &Peer) -> Result<Message<U, T, S>>
        where U: serde::de::DeserializeOwned,
              T: serde::de::DeserializeOwned,
              S: serde::de::DeserializeOwned,
    {
        let open_bytes = curve25519xsalsa20poly1305::open_precomputed(self.sealed_body(), self.nonce(), peer_from.precomputed())
            .map_err(|_| Error::MessageOpenFailed)?;
        let msg: Message<U, T, S> = ser::deserialize(&open_bytes[..])?;
        Ok(msg)
    }
}

/// An outer message type. This allows sending specific messages unencrypted,
/// but enforces encryption for all other messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageWrapped {
    /// Send a sealed message.
    Sealed(MessageSealed),
    /// Send an unencrypted "hi, I'm Dad" message, containing our pubkey. This
    /// allows the peer we send to to initiate encrypted communication.
    Init(PeerPubkey),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::Event,
        peer::Keypair,
    };

    #[test]
    fn can_gen_nonce() {
        let nonce1 = gen_nonce();
        let nonce2 = gen_nonce();
        // won't always pass, but probably fine 99.99999999% of the time
        assert!(nonce1 != nonce2);
    }

    #[test]
    fn can_seal_and_open() {
        let seed_send = vec![13; curve25519xsalsa20poly1305::SEEDBYTES];
        let seed_recv = vec![42; curve25519xsalsa20poly1305::SEEDBYTES];
        let keypair_send = Keypair::new_with_seed(&seed_send).unwrap();
        let keypair_recv = Keypair::new_with_seed(&seed_recv).unwrap();
        let peer_send = Peer::new("jerry".into(), keypair_send.to_pubkey(), keypair_recv.seckey());
        let peer_recv = Peer::new("larry".into(), keypair_recv.to_pubkey(), keypair_send.seckey());

        let body = Event::Hello;
        //let seal_key = curve25519xsalsa20poly1305::precompute(keypair_recv.pubkey(), keypair_send.seckey());
        //let sealed = MessageSealed::create(&seal_key, msgid.clone(), body, vec![]).unwrap();
        let sealed = MessageSealed::create::<(), String, String>(peer_send.pubkey(), &peer_recv, Utc::now(), gen_nonce(), body, vec![]).unwrap();

        let message = sealed.open::<(), (), ()>(&peer_send).unwrap();

        assert_eq!(message.body(), &Event::Hello);

        let seed_send2 = vec![14; curve25519xsalsa20poly1305::SEEDBYTES];
        let seed_recv2 = vec![43; curve25519xsalsa20poly1305::SEEDBYTES];
        let keypair_send2 = Keypair::new_with_seed(&seed_send2).unwrap();
        let keypair_recv2 = Keypair::new_with_seed(&seed_recv2).unwrap();
        let peer_send2 = Peer::new("jerry".into(), keypair_send2.to_pubkey(), keypair_recv2.seckey());
        let peer_recv2 = Peer::new("larry".into(), keypair_recv2.to_pubkey(), keypair_send2.seckey());

        // check bad decryption pairs fail
        assert_eq!(sealed.open::<(), (), ()>(&peer_send2), Err(Error::MessageOpenFailed));
        assert_eq!(sealed.open::<(), (), ()>(&peer_recv2), Err(Error::MessageOpenFailed));
    }
}

