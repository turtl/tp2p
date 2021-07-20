use chrono::{DateTime, Utc};
use crate::{
    error::{Error, Result},
    event::Event,
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

/// Tracks our different protocol versions
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
pub enum MessageProtocolVersion {
    V1,
}

impl MessageProtocolVersion {
    /// Returns the current/default protocol version.
    pub fn default() -> Self {
        Self::V1
    }
}

/// A message ID. Allows us to distinguish messages.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct MessageID {
    /// Timestamp of the ID
    timestamp: DateTime<Utc>,
    /// Random nonce for each message lol
    nonce: Nonce,
    /// Protocol version. Makes sure message formats can change in the future
    /// but remain backwards compatible.
    version: MessageProtocolVersion,
}

impl MessageID {
    /// Create a new MessageID
    fn new(timestamp: DateTime<Utc>, nonce: Nonce) -> Self {
        Self {
            timestamp,
            nonce,
            version: MessageProtocolVersion::default(),
        }
    }
}

/// A message, the basis for communication between peers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct Message {
    /// The id of this message. Hopefully unique.
    ///
    /// The receiver should store the message IDs of all messages it has
    /// responded to within the time range T and ignore any repeats. This
    /// eliminates duplicates/replays.
    id: MessageID,
    /// Holds the MessageIDs of any parent messages (generally one, but can be
    /// multiple). This helps with partial ordering of events.
    parents: Vec<MessageID>,
    /// Effectively gives us a loose ordering of events. This message's depth
    /// value should always be greater than the messages it is referencing.
    depth: u64,
    /// "This is the actual message body," he said with a boyish grin.
    ///
    /// All messages are `Event`s.
    body: Event,
}

impl Message {
    /// Create a new message
    pub fn new(id: MessageID, body: Event, parents: Vec<&Message>) -> Self {
        let depth = parents.iter()
            .fold(0, |acc, x| std::cmp::max(acc, x.depth));
        Self {
            id,
            parents: parents.iter().map(|x| x.id().clone()).collect::<Vec<_>>(),
            depth: depth + 1,
            body,
        }
    }

    /// Sort a set of messages.
    pub fn sort(messages: &Vec<Message>) -> Vec<Message> {
        let mut sorted = messages.clone();
        sorted.sort_by(|a, b| {
            if a.depth == b.depth {
                a.id.cmp(&b.id)
            } else {
                a.depth.cmp(&b.depth)
            }
        });
        sorted
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
    pub fn create(pubkey_sender: &PeerPubkey, peer_to: &Peer, now: DateTime<Utc>, nonce: Nonce, body: Event, parents: Vec<&Message>) -> Result<Self> {
        let id = MessageID::new(now, nonce);
        let msg = Message::new(id.clone(), body, parents);
        let serialized_body = ser::serialize(&msg)?;
        let sealed = curve25519xsalsa20poly1305::seal_precomputed(&serialized_body, id.nonce(), peer_to.precomputed());
        let sealed_message = Self {
            nonce: id.nonce().clone(),
            pubkey_sender: pubkey_sender.clone(),
            sealed_body: sealed,
        };
        Ok(sealed_message)
    }

    /// Open a sealed message and return the inner Message object.
    pub fn open(&self, peer_from: &Peer) -> Result<Message> {
        let open_bytes = curve25519xsalsa20poly1305::open_precomputed(self.sealed_body(), self.nonce(), peer_from.precomputed())
            .map_err(|_| Error::MessageOpenFailed)?;
        let msg: Message = ser::deserialize(&open_bytes[..])?;
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
        Mode,
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
        let peer_send = Peer::new(Mode::Agent, "jerry".into(), keypair_send.to_pubkey(), keypair_recv.seckey());
        let peer_recv = Peer::new(Mode::Agent, "larry".into(), keypair_recv.to_pubkey(), keypair_send.seckey());

        let body = Event::Hello;
        //let seal_key = curve25519xsalsa20poly1305::precompute(keypair_recv.pubkey(), keypair_send.seckey());
        //let sealed = MessageSealed::create(&seal_key, msgid.clone(), body, vec![]).unwrap();
        let sealed = MessageSealed::create(peer_send.pubkey(), &peer_recv, Utc::now(), gen_nonce(), body, vec![]).unwrap();

        let message = sealed.open(&peer_send).unwrap();

        assert_eq!(message.body(), &Event::Hello);

        let seed_send2 = vec![14; curve25519xsalsa20poly1305::SEEDBYTES];
        let seed_recv2 = vec![43; curve25519xsalsa20poly1305::SEEDBYTES];
        let keypair_send2 = Keypair::new_with_seed(&seed_send2).unwrap();
        let keypair_recv2 = Keypair::new_with_seed(&seed_recv2).unwrap();
        let peer_send2 = Peer::new(Mode::Agent, "jerry".into(), keypair_send2.to_pubkey(), keypair_recv2.seckey());
        let peer_recv2 = Peer::new(Mode::Agent, "larry".into(), keypair_recv2.to_pubkey(), keypair_send2.seckey());

        // check bad decryption pairs fail
        assert_eq!(sealed.open(&peer_send2), Err(Error::MessageOpenFailed));
        assert_eq!(sealed.open(&peer_recv2), Err(Error::MessageOpenFailed));
    }

    #[test]
    fn message_ordering() {
        let seed = vec![69; curve25519xsalsa20poly1305::SEEDBYTES];
        let keypair = Keypair::new_random();
        let pubkey = keypair.to_pubkey();
        let pubkey_sender = Keypair::new_random().to_pubkey();
        
        macro_rules! message_id {
            () => {
                MessageID::new(Utc::now(), gen_nonce())
            }
        }

        let msgid = message_id!();
        let body = Event::Hello;
        let msg1 = Message::new(msgid, body.clone(), vec![]);

        let msgid = message_id!();
        let body = Event::Hello;
        let msg2 = Message::new(msgid, body.clone(), vec![&msg1]);

        let msgid = message_id!();
        let body = Event::Hello;
        let msg3 = Message::new(msgid, body.clone(), vec![&msg1]);

        let msgid = message_id!();
        let body = Event::Hello;
        let msg4 = Message::new(msgid, body.clone(), vec![&msg2, &msg3]);

        let msgid = message_id!();
        let body = Event::Hello;
        let msg5 = Message::new(msgid, body.clone(), vec![&msg4]);

        assert_eq!(msg1.depth(), &1);
        assert_eq!(msg2.depth(), &2);
        assert_eq!(msg3.depth(), &2);
        assert_eq!(msg4.depth(), &3);
        assert_eq!(msg5.depth(), &4);
        assert_eq!(msg1.parents(), &vec![]);
        assert_eq!(msg2.parents(), &vec![msg1.id().clone()]);
        assert_eq!(msg3.parents(), &vec![msg1.id().clone()]);
        assert_eq!(msg4.parents(), &vec![msg2.id().clone(), msg3.id().clone()]);
        assert_eq!(msg5.parents(), &vec![msg4.id().clone()]);

        let messages = vec![msg3.clone(), msg2.clone(), msg1.clone(), msg5.clone(), msg4.clone()];
        let sorted = Message::sort(&messages);
        assert_eq!(sorted, vec![msg1, msg2, msg3, msg4, msg5]);
    }
}

