use chrono::{naive::NaiveDateTime, Utc};
use crate::{
    error::{Error, Result},
    ser,
};
use serde_derive::{Serialize, Deserialize};
use sodiumoxide::{
    crypto::box_::curve25519xsalsa20poly1305::{self, PublicKey, SecretKey, PrecomputedKey, Nonce},
};

/// An ID object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MessageID {
    /// Timestamp of the ID
    #[serde(with = "chrono::naive::serde::ts_nanoseconds")]
    timestamp: NaiveDateTime,
    /// public key of the recipient
    pubkey: PublicKey,
    /// Random nonce for each message lol
    nonce: Nonce,
}

impl MessageID {
    /// Create a new MessageID
    pub fn new(timestamp: NaiveDateTime, recipient_pubkey: PublicKey, nonce: Nonce) -> Self {
        Self {
            timestamp,
            pubkey: recipient_pubkey,
            nonce,
        }
    }

    /// Create a new MessageID with the timestamp set to the current time
    pub fn new_now(recipient_pubkey: PublicKey, nonce: Nonce) -> Self {
        Self::new(Utc::now().naive_utc(), recipient_pubkey, nonce)
    }

    /// Does EXACTLY what the name says. Creates a new MessageID with a random
    /// nonce and the current system time.
    pub fn new_now_nonce(recipient_pubkey: PublicKey) -> Self {
        let nonce = curve25519xsalsa20poly1305::gen_nonce();
        Self::new(Utc::now().naive_utc(), recipient_pubkey, nonce)
    }

    /// Get the id's timestamp
    pub fn timestamp(&self) -> &NaiveDateTime {
        &self.timestamp
    }

    /// Get the id's pubkey
    pub fn pubkey(&self) -> &PublicKey {
        &self.pubkey
    }

    /// Get the id's nonce
    pub fn nonce(&self) -> &Nonce {
        &self.nonce
    }
}

/// A message, the basis for communication between peers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Serialized, must be deserialized into whatever you want.
    body: Vec<u8>,
}

impl Message {
    /// Get this message's MessageID
    pub fn id(&self) -> &MessageID {
        &self.id
    }

    /// Get this message's depth value
    pub fn depth(&self) -> u64 {
        self.depth
    }

    /// Get this message's parents
    pub fn parents(&self) -> &Vec<MessageID> {
        &self.parents
    }

    /// Get this message's body bytes
    pub fn body(&self) -> &Vec<u8> {
        &self.body
    }

    /// Create a new message
    pub fn new(id: MessageID, body: Vec<u8>, parents: Vec<&Message>) -> Self {
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

/// Tracks our different protocol versions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageProtocolVersion {
    V1,
}

impl MessageProtocolVersion {
    /// Returns the current/default protocol version.
    pub fn default() -> Self {
        Self::V1
    }
}

/// A sealed message. Can only be opened using the private key matching the
/// public key it was encrypted with. Also, the contents must match the
/// signature of the private key that signed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSealed {
    /// Protocol version. Makes sure message formats can change in the future
    /// but remain backwards compatible.
    version: MessageProtocolVersion,
    /// The id of the message.
    id: MessageID,
    /// The encryped body of our message. This deserializes to a [Message](crate::message::Message).
    sealed_body: Vec<u8>,
}

impl MessageSealed {
    /// Creates a new signed and encrypted message in reply to one or more other
    /// messages.
    pub fn create(seal_key: &PrecomputedKey, id: MessageID, body: Vec<u8>, parents: Vec<&Message>) -> Result<Self> {
        let msg = Message::new(id.clone(), body, parents);
        let serialized_body = ser::serialize(&msg)?;
        let sealed = curve25519xsalsa20poly1305::seal_precomputed(&serialized_body, &id.nonce, seal_key);
        let sealed_message = Self {
            version: MessageProtocolVersion::default(),
            id,
            sealed_body: sealed,
        };
        Ok(sealed_message)
    }

    /// Open a sealed message and return the inner message.
    pub fn open(&self, open_key: &PrecomputedKey) -> Result<Message> {
        let open_bytes = curve25519xsalsa20poly1305::open_precomputed(&self.sealed_body, &self.id.nonce(), open_key)
            .map_err(|_| Error::MessageOpenFailed)?;
        let msg: Message = ser::deserialize(&open_bytes[..])?;
        Ok(msg)
    }

    /// Get this message's protocol version
    pub fn version(&self) -> &MessageProtocolVersion {
        &self.version
    }

    /// Get this message's id
    pub fn id(&self) -> &MessageID {
        &self.id
    }

    /// Get this message's sealed body
    pub fn sealed_body(&self) -> &Vec<u8> {
        &self.sealed_body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_seal_and_open() {
        let seed_send = vec![13; curve25519xsalsa20poly1305::SEEDBYTES];
        let seed_recv = vec![42; curve25519xsalsa20poly1305::SEEDBYTES];
        let (pubkey_send, seckey_send) = curve25519xsalsa20poly1305::keypair_from_seed(&curve25519xsalsa20poly1305::Seed::from_slice(&seed_send).unwrap());
        let (pubkey_recv, seckey_recv) = curve25519xsalsa20poly1305::keypair_from_seed(&curve25519xsalsa20poly1305::Seed::from_slice(&seed_recv).unwrap());

        let msgid = MessageID::new_now_nonce(pubkey_recv);
        let body = Vec::from(String::from("I'm here for the scraps").as_bytes());
        let seal_key = curve25519xsalsa20poly1305::precompute(&pubkey_recv, &seckey_send);
        let sealed = MessageSealed::create(&seal_key, msgid.clone(), body, vec![]).unwrap();

        let open_key = curve25519xsalsa20poly1305::precompute(&pubkey_send, &seckey_recv);
        let message = sealed.open(&open_key).unwrap();

        assert_eq!(message.id(), &msgid);
        assert_eq!(String::from_utf8(message.body().clone()).unwrap().as_str(), "I'm here for the scraps");

        let seed_send2 = vec![14; curve25519xsalsa20poly1305::SEEDBYTES];
        let seed_recv2 = vec![43; curve25519xsalsa20poly1305::SEEDBYTES];
        let (pubkey_send2, seckey_send2) = curve25519xsalsa20poly1305::keypair_from_seed(&curve25519xsalsa20poly1305::Seed::from_slice(&seed_send2).unwrap());
        let (pubkey_recv2, seckey_recv2) = curve25519xsalsa20poly1305::keypair_from_seed(&curve25519xsalsa20poly1305::Seed::from_slice(&seed_recv2).unwrap());

        // check bad decryption key fails
        let open_key2 = curve25519xsalsa20poly1305::precompute(&pubkey_send, &seckey_recv2);
        assert_eq!(sealed.open(&open_key2), Err(Error::MessageOpenFailed));

        // check bad signing key fails
        let open_key3 = curve25519xsalsa20poly1305::precompute(&pubkey_send2, &seckey_recv);
        assert_eq!(sealed.open(&open_key3), Err(Error::MessageOpenFailed));
    }

    #[test]
    fn message_ordering() {
        let seed = vec![69; curve25519xsalsa20poly1305::SEEDBYTES];
        let (pubkey, seckey) = curve25519xsalsa20poly1305::keypair_from_seed(&curve25519xsalsa20poly1305::Seed::from_slice(&seed).unwrap());

        let msgid = MessageID::new_now_nonce(pubkey.clone());
        let body = Vec::from(String::from("No it's not like horse!!").as_bytes());
        let msg1 = Message::new(msgid, body.clone(), vec![]);

        let msgid = MessageID::new_now_nonce(pubkey.clone());
        let body = Vec::from(String::from("PFFFFFFT").as_bytes());
        let msg2 = Message::new(msgid, body.clone(), vec![&msg1]);

        let msgid = MessageID::new_now_nonce(pubkey.clone());
        let body = Vec::from(String::from("PFFFFFFFFFFT").as_bytes());
        let msg3 = Message::new(msgid, body.clone(), vec![&msg1]);

        let msgid = MessageID::new_now_nonce(pubkey.clone());
        let body = Vec::from(String::from("PFFFFFFFFFFFFFFFFT").as_bytes());
        let msg4 = Message::new(msgid, body.clone(), vec![&msg2, &msg3]);

        let msgid = MessageID::new_now_nonce(pubkey.clone());
        let body = Vec::from(String::from("d-duh").as_bytes());
        let msg5 = Message::new(msgid, body.clone(), vec![&msg4]);

        assert_eq!(msg1.depth(), 1);
        assert_eq!(msg2.depth(), 2);
        assert_eq!(msg3.depth(), 2);
        assert_eq!(msg4.depth(), 3);
        assert_eq!(msg5.depth(), 4);
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

