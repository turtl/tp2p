use chrono::{DateTime, Utc};
use crate::{
    action::{Action, SyncAction},
    error::{Error, Result},
    event::Event,
    identity::{Pubkey as IdentityPubkey},
    message::{self, TopicGraphEntry, Message, MessageSealed, MessageWrapped},
    peer::{Keypair, Pubkey as PeerPubkey, Peer, PeerInfo, PeeringConfig},
    subscription::Subscription,
};
use getset::Getters;
use serde_derive::{Serialize, Deserialize};
use std::{
    collections::HashMap,
    fmt::{self, Debug},
    net::SocketAddr,
    ops::Deref,
};
use tracing::{debug, info, warn};

/// Holds information about a connection to a peer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Getters, Serialize, Deserialize)]
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

impl fmt::Display for ConnectionInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "addr::{}", self.inner())
    }
}

impl From<SocketAddr> for ConnectionInfo {
    fn from(sock: SocketAddr) -> Self {
        Self {
            inner: sock,
        }
    }
}

/// A structure that holds information about a peer we're just starting to open
/// comms with.
#[derive(Debug, Clone, Getters, Serialize, Deserialize)]
#[getset(get = "pub(crate)")]
pub struct PendingPeer {
    /// Connection info
    connection_info: ConnectionInfo,
    /// When the pending peer was created
    created: DateTime<Utc>,
}

impl PendingPeer {
    fn new(connection_info: ConnectionInfo, now: DateTime<Utc>) -> Self {
        Self {
            connection_info,
            created: now,
        }
    }
}

/// The main interface to the syncing system: this produces events for both
/// communicating to other peers and interacting with the database.
#[derive(Debug, Clone, Getters)]
#[getset(get = "pub")]
pub struct Sync<T> {
    /// How we want to deal with peering requests.
    peering_config: PeeringConfig,
    /// The name of our peer.
    name: String,
    /// Our comm secret keypair. Used to encrypt/decrypt messages to peers.
    keypair: Keypair,
    /// A list of peers we are in contact with. They do not necessarily need to
    /// be active.
    peers: HashMap<PeerPubkey, PeerInfo>,
    /// A list of subscriptions that peers have.
    subscriptions: HashMap<T, Vec<PeerInfo>>,
    /// Track outgoing peer contact initiations so we don't send PeerInit to any
    /// random dork who Hello's us.
    pending_peer_init: Vec<PendingPeer>,
    /// Identities this peer manages
    identities: Vec<IdentityPubkey>,
}

impl<T> Sync<T> {
    /// Find a peer in our peering list by pubkey.
    fn find_peer(&self, pubkey: &PeerPubkey, active: Option<bool>) -> Option<&PeerInfo> {
        self.peers.get(pubkey)
            .and_then(|peer_info| {
                let active_bool = match active {
                    Some(active_val) => peer_info.active() == &active_val,
                    None => true,
                };
                if active_bool {
                    Some(peer_info)
                } else {
                    None
                }
            })
    }

    /// Find an existing outgoing peer request.
    fn find_pending_peer_init(&self, connection_info: &ConnectionInfo) -> Option<&PendingPeer> {
        self.pending_peer_init.iter()
            .find(|p| p.connection_info() == connection_info)
    }

    /// Helper for creating messages.
    fn create_sealed_message<U, S>(&self, to: &Peer, body: Event<U, T, S>, topic_graph: Vec<TopicGraphEntry<T>>, now: &DateTime<Utc>) -> Result<MessageWrapped>
        where U: serde::Serialize,
              T: serde::Serialize,
              S: serde::Serialize,
    {
        let sealed = MessageSealed::create(&self.keypair().to_pubkey(), &to, now.clone(), message::gen_nonce(), body, topic_graph)?;
        let wrapped = MessageWrapped::Sealed(sealed);
        Ok(wrapped)
    }

    /// Determines if a peer is subscribed to a given topic
    fn is_peer_subscribed_to_topic(&self, topic: &T, peer: &Peer) -> bool
        where T: std::hash::Hash + std::cmp::Eq,
    {
        self.subscriptions.get(topic)
            .and_then(|peers| peers.iter().find(|p| p.peer() == peer))
            .is_some()
    }

    /// Makes sure a message contains at least one topic that the given peer is
    /// subscribed to. If not, returns false.
    fn filter_message_by_peer<U, S>(&self, peer: &Peer, message: &Message<U, T, S>) -> bool
        where T: std::hash::Hash + std::cmp::Eq,
    {
        for topic in message.topic_graph() {
            if self.is_peer_subscribed_to_topic(topic.topic(), peer) {
                return true;
            }
        }
        false
    }

    /// Create a new Sync object
    pub fn new(name: String, keypair: Keypair, peering_config: PeeringConfig) -> Self {
        Self {
            name,
            keypair,
            peers: HashMap::new(),
            subscriptions: HashMap::new(),
            pending_peer_init: Vec::new(),
            identities: Vec::new(),
            peering_config,
        }
    }

    /// Take a set of [SyncAction](crate::action::SyncAction)s and apply them to
    /// this `Sync` object.
    pub fn apply_sync_actions(&mut self, actions: Vec<SyncAction<T>>)
        where T: Eq + std::hash::Hash
    {
        for action in actions {
            match action {
                SyncAction::SetPeer(peer_info) => {
                    self.peers.insert(peer_info.peer().pubkey().clone(), peer_info);
                }
                SyncAction::UnsetPeer(pubkey) => {
                    self.peers.remove(&pubkey);
                }
                SyncAction::SetPendingPeer(pending_peer) => {
                    let existing_req = self.find_pending_peer_init(pending_peer.connection_info());
                    if existing_req.is_none() {
                        self.pending_peer_init.push(pending_peer);
                    }
                }
                SyncAction::UnsetPendingPeer(connection_info) => {
                    self.pending_peer_init.retain(|p| {
                        !(p.connection_info() == &connection_info)
                    });
                }
                SyncAction::SetSubscription(topic, peer_info) => {
                    let entry = self.subscriptions.entry(topic).or_insert(vec![]);
                    if !entry.iter().find(|x| x.peer() == peer_info.peer()).is_some() {
                        entry.push(peer_info);
                    }
                }
                SyncAction::UnsetSubscription(topic, peer_info) => {
                    if let Some(peers) = self.subscriptions.get_mut(&topic) {
                        peers.retain(|x| x.peer() != peer_info.peer());
                    }
                    // take out the trash
                    if self.subscriptions.get(&topic).map(|x| x.len()) == Some(0) {
                        self.subscriptions.remove(&topic);
                    }
                }
            }
        }
    }

    /// This method should be called periodically (like every 60s or something)
    /// to perform basic maintenance on the sync object.
    ///
    /// Mainly we ping our peers and prune our outgoing peer init trackers.
    #[tracing::instrument(skip(self))]
    pub fn tick<U, S>(&self, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: serde::Serialize,
              S: serde::Serialize,
    {
        let mut actions = Vec::<Action<U, T, S>>::new();
        for peer_info in self.peers().values() {
            let last_ping_seconds = (*now - *peer_info.last_ping()).num_seconds();
            if last_ping_seconds < 60 {
                continue;
            }
            if last_ping_seconds > 180 && !peer_info.active() {
                // if the peer isn't confirmed and isn't responding to pings,
                // axe it
                info!(peer = %peer_info.peer().pubkey(), forget = true);
                actions.push(Action::Sync(SyncAction::UnsetPeer(peer_info.peer().pubkey().clone())));
            } else {
                let body = Event::<U, T, S>::Ping;
                let message = self.create_sealed_message(peer_info.peer(), body, vec![], now)?;
                debug!(peer = %peer_info.peer().pubkey(), ping = true);
                actions.push(Action::MessageSend(peer_info.connection_info().clone(), message));
            }
        }

        for pending in self.pending_peer_init() {
            let seconds_since_create = (*now - *pending.created()).num_seconds();
            if seconds_since_create < 120 {
                continue;
            }
            debug!("sync.tick() -- unsetting inactive pending peer: {}", pending.connection_info().inner());
            actions.push(Action::Sync(SyncAction::UnsetPendingPeer(pending.connection_info().clone())));
        }
        Ok(actions)
    }

    /// Handles [`MessageWrapped::Init`] messages. All other messages (ie, [`MessageSealed`])
    /// objects should be handed to `unwrap_incoming_message()`. Or else.
    #[tracing::instrument(skip(message_wrapped))]
    pub fn process_init_message<U, S>(&self, message_wrapped: &MessageWrapped, connection_info: &ConnectionInfo, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: Debug + serde::Serialize,
              S: serde::Serialize,
    {
        match message_wrapped {
            // unwrap the sealed message and hand it back
            MessageWrapped::Sealed(..) => Err(Error::MessageNotInit)?,
            // a peer is initating unencrypted communication. send them back a
            // `Hello` so they can have our key, then return.
            MessageWrapped::Init(ref pubkey) => {
                let new_peer = Peer::tmp(pubkey, self.keypair().seckey());
                let body = Event::<U, T, S>::Hello;
                let outgoing = MessageSealed::create(&self.keypair().to_pubkey(), &new_peer, now.clone(), message::gen_nonce(), body, Vec::<TopicGraphEntry<T>>::new())?;
                let outgoing_wrapped = MessageWrapped::Sealed(outgoing);
                info!("sync.process_init_message() -- sending hello to {} // {:?}", connection_info.inner(), pubkey);
                Ok(vec![
                    Action::Sync(SyncAction::SetPeer(PeerInfo::new(new_peer.clone(), connection_info.clone(), false, now.clone()))),
                    Action::MessageSend(connection_info.clone(), outgoing_wrapped),
                ])
            }
        }
    }

    /// Unwrap an incoming message and return it, and any supporting information
    /// about it, to the caller. The caller then must inspect the returned event
    /// object to determine which supporting function (`process_event_*`) to call
    /// (which returns a set of actions to take).
    #[tracing::instrument(skip(self, message), fields(peer = %message.pubkey_sender()))]
    pub fn unwrap_incoming_message<U, S>(&self, message: &MessageSealed) -> Result<Message<U, T, S>>
        where U: serde::de::DeserializeOwned,
              T: serde::de::DeserializeOwned,
              S: serde::de::DeserializeOwned,
    {
        let peer_from_maybe = self.find_peer(message.pubkey_sender(), None);
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
        }

        debug!("sync.unwrap_incoming_message() -- successfully unwrapped from {:?}", message.pubkey_sender());
        Ok(message_opened)
    }

    /// Call this when you have an [`Event::Hello`] object you got from the
    /// [`unwrap_incoming_message`] call.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender, addr = %connection_info))]
    pub fn process_event_hello<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, connection_info: &ConnectionInfo, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: serde::Serialize,
              S: serde::Serialize,
    {
        if !matches!(message_opened.body(), &Event::Hello) {
            Err(Error::EventMismatch)?;
        }
        info!("sync.process_event_hello() -- got hello from {} // {:?}", connection_info.inner(), pubkey_sender);
        // don't process a `Helloooo` unless we initiated it.
        if self.find_pending_peer_init(connection_info).is_none() {
            warn!("sync.process_event_hello() -- received rogue Hello from {} // {:?}", connection_info.inner(), pubkey_sender);
            return Ok(vec![]);
        }
        let mut actions: Vec<Action<U, T, S>> = Vec::with_capacity(2);
        let new_peer = Peer::tmp(&pubkey_sender, self.keypair().seckey());
        // if we haven't already, add this peer to our list (as inactive)
        actions.push(Action::Sync(SyncAction::SetPeer(PeerInfo::new(new_peer.clone(), connection_info.clone(), false, now.clone()))));
        // now that the peer is in our stupid list, we can remove the
        // pending_peer_init entry
        actions.push(Action::Sync(SyncAction::UnsetPendingPeer(connection_info.clone())));

        let body = Event::<U, T, S>::PeerInit { name: self.name().clone() };
        let message = self.create_sealed_message(&new_peer, body, vec![], now)?;
        actions.push(Action::MessageSend(connection_info.clone(), message));
        Ok(actions)
    }

    /// Call this to process an [`Event::PeerInit`] message.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender, addr = %connection_info))]
    pub fn process_event_peer_init<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, connection_info: &ConnectionInfo, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>>
        where U: Clone + serde::Serialize,
              T: Clone + serde::Serialize,
              S: Clone + serde::Serialize,
    {
        match message_opened.body() {
            Event::PeerInit { name } => {
                let access = self.peering_config.can_confirm(pubkey_sender);
                match access {
                    Some(true) => {
                        info!("sync.process_event_peer_init() -- confirmed: {} // {:?}", connection_info.inner(), pubkey_sender);
                        self.peer_confirm::<U, S>(pubkey_sender, &name, now)
                    }
                    // TODO: we have denied peering with this node...do we really need
                    // to do anything special here? seems like taking no action is the
                    // best course of action.
                    Some(false) => {
                        warn!("sync.process_event_peer_init() -- attempt denied: {} // {:?}", connection_info.inner(), pubkey_sender);
                        Ok(vec![])
                    }
                    None => {
                        info!("sync.process_event_peer_init() -- need confirmation: {} // {:?}", connection_info.inner(), pubkey_sender);
                        Ok(vec![Action::Confirm(message_opened.body().clone())])
                    }
                }
            }
            _ => Err(Error::EventMismatch)?,
        }
    }

    /// Call this to process an [`Event::PeerConfirm`] message.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender))]
    pub fn process_event_peer_confirm<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>> {
        match message_opened.body() {
            Event::PeerConfirm { name } => {
                let peer_info = self.find_peer(pubkey_sender, None)
                    .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
                let mut peer_mod = peer_info.clone();
                peer_mod.peer_mut().set_name(name.to_string());
                peer_mod.set_active(true);
                peer_mod.set_last_ping(now.clone());
                Ok(vec![
                    Action::Sync(SyncAction::SetPeer(peer_mod)),
                ])
            }
            _ => Err(Error::EventMismatch)?,
        }
    }

    /// Call this to process an [`Event::Ping`] message.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender))]
    pub fn process_event_ping<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: serde::Serialize,
              S: serde::Serialize,
    {
        if !matches!(message_opened.body(), &Event::Ping) {
            Err(Error::EventMismatch)?;
        }
        let peer_info = self.find_peer(pubkey_sender, None)
            .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
        info!("sync.process_event_ping() -- got ping from {} // {:?}", peer_info.connection_info().inner(), pubkey_sender);
        let mut actions: Vec<Action<U, T, S>> = Vec::with_capacity(2);

        // mark the peer as pinged
        let mut peer_mod = peer_info.clone();
        peer_mod.set_last_ping(now.clone());
        actions.push(Action::Sync(SyncAction::SetPeer(peer_mod)));

        // send a pong
        let body = Event::<U, T, S>::Pong;
        let message = self.create_sealed_message(peer_info.peer(), body, vec![], now)?;
        actions.push(Action::MessageSend(peer_info.connection_info().clone(), message));
        Ok(actions)
    }

    /// Call this to process an [`Event::Pong`] message.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender))]
    pub fn process_event_pong<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>> {
        if !matches!(message_opened.body(), &Event::Pong) {
            Err(Error::EventMismatch)?;
        }
        // mark the peer as pinged
        let peer_info = self.find_peer(pubkey_sender, None)
            .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
        info!("sync.process_event_ping() -- got pong from {} // {:?}", peer_info.connection_info().inner(), pubkey_sender);
        let mut peer_mod = peer_info.clone();
        peer_mod.set_last_ping(now.clone());
        Ok(vec![Action::Sync(SyncAction::SetPeer(peer_mod))])
    }

    /// Call this to process an [`Event::QueryMessagesByID`] message. Requires
    /// that the queried messages be passed in.
    #[tracing::instrument(skip(self, message_opened, messages), fields(peer = %pubkey_sender))]
    pub fn process_event_query_messages_by_id<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, messages: &Vec<Message<U, T, S>>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: std::hash::Hash + std::cmp::Eq + serde::Serialize,
              S: serde::Serialize,
    {
        if messages.len() == 0 {
            return Ok(vec![]);
        }
        match message_opened.body() {
            Event::QueryMessagesByID { .. } => {
                let peer_info = self.find_peer(pubkey_sender, None)
                    .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
                messages.iter()
                    // only send messages that this peer has subscriptions to
                    .filter(|m| self.filter_message_by_peer(peer_info.peer(), m))
                    .map(|m| MessageSealed::seal(&self.keypair.to_pubkey(), peer_info.peer(), m))
                    .map(|msg_res| {
                        msg_res.map(|sealed| {
                            Action::<U, T, S>::MessageSend(peer_info.connection_info().clone(), MessageWrapped::Sealed(sealed))
                        })
                    })
                    .collect::<Result<Vec<Action<U, T, S>>>>()
            }
            _ => Err(Error::EventMismatch)?,
        }
    }

    /// Call this to process an [`Event::QueryMessagesByDepth`] message. Requires
    /// that the queried messages be passed in.
    #[tracing::instrument(skip(self, message_opened, messages), fields(peer = %pubkey_sender))]
    pub fn process_event_query_messages_by_depth<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, messages: &Vec<Message<U, T, S>>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: std::hash::Hash + std::cmp::Eq + serde::Serialize,
              S: serde::Serialize,
    {
        if messages.len() == 0 {
            return Ok(vec![]);
        }
        match message_opened.body() {
            Event::QueryMessagesByDepth { .. } => {
                let peer_info = self.find_peer(pubkey_sender, None)
                    .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
                messages.iter()
                    // only send messages that this peer has subscriptions to
                    .filter(|m| self.filter_message_by_peer(peer_info.peer(), m))
                    .map(|m| MessageSealed::seal(&self.keypair.to_pubkey(), peer_info.peer(), m))
                    .map(|msg_res| {
                        msg_res.map(|sealed| {
                            Action::<U, T, S>::MessageSend(peer_info.connection_info().clone(), MessageWrapped::Sealed(sealed))
                        })
                    })
                    .collect::<Result<Vec<Action<U, T, S>>>>()
            }
            _ => Err(Error::EventMismatch)?,
        }
    }

    /// Call this to process an [`Event::Subscribe`] message. Requires
    /// that the queried messages be passed in.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender))]
    pub fn process_event_subscribe<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey) -> Result<Vec<Action<U, T, S>>>
        where T: Clone,
              S: Subscription<T>,
    {
        match message_opened.body() {
            Event::Subscribe(subscriptions) => {
                let peer_info = self.find_peer(pubkey_sender, None)
                    .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
                let actions = subscriptions.iter()
                    .filter_map(|subscription| {
                        subscription.verify()
                            .map(|topic| Action::Sync(SyncAction::SetSubscription(topic.clone(), peer_info.clone())))
                    })
                    .collect::<Vec<_>>();
                Ok(actions)
            }
            _ => Err(Error::EventMismatch)?,
        }
    }

    /// Call this to process an [`Event::Unsubscribe`] message. Requires
    /// that the queried messages be passed in.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender))]
    pub fn process_event_unsubscribe<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey) -> Result<Vec<Action<U, T, S>>>
        where T: Clone,
    {
        match message_opened.body() {
            Event::Unsubscribe(topics) => {
                let peer_info = self.find_peer(pubkey_sender, None)
                    .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
                let actions = topics.iter()
                    .map(|topic| Action::Sync(SyncAction::UnsetSubscription(topic.clone(), peer_info.clone())))
                    .collect::<Vec<_>>();
                Ok(actions)
            }
            _ => Err(Error::EventMismatch)?,
        }
    }

    /// Call this to process an [`Event::QuerySubscriptions`] message. Requires
    /// that the queried messages be passed in.
    #[tracing::instrument(skip(self, message_opened), fields(peer = %pubkey_sender))]
    pub fn process_event_query_subscriptions<U, S>(&self, message_opened: &Message<U, T, S>, pubkey_sender: &PeerPubkey, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: std::hash::Hash + Eq + Clone + serde::Serialize,
              S: serde::Serialize,
    {
        match message_opened.body() {
            Event::QuerySubscriptions => {
                let peer_info = self.find_peer(pubkey_sender, None)
                    .ok_or(Error::PeerNotFound(pubkey_sender.clone()))?;
                let mut subscriptions = HashMap::new();
                for (topic, peers) in &self.subscriptions {
                    if peers.iter().find(|p| p.peer() == peer_info.peer()).is_some() {
                        subscriptions.insert(topic.clone(), true);
                    }
                }
                let topics = subscriptions.keys()
                    .map(|t| t.clone())
                    .collect::<Vec<_>>();
                let body = Event::<U, T, S>::ActiveSubscriptions(topics);
                let sealed = self.create_sealed_message(peer_info.peer(), body, vec![], now)?;
                Ok(vec![Action::MessageSend(peer_info.connection_info().clone(), sealed)])
            }
            _ => Err(Error::EventMismatch),
        }
    }

    /// Initialize outgoing "relations"  ;) ;) with another peer ;)
    #[tracing::instrument(skip(self), fields(addr = %addr))]
    pub fn init_comm<U, S>(&self, addr: &str, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>> {
        info!("sync.init_comm() -- being relations with {} lol", addr);
        let connection_info = ConnectionInfo::new(addr)?;
        let message_wrapped = MessageWrapped::Init(self.keypair.to_pubkey().clone());

        Ok(vec![
            Action::<U, T, S>::Sync(SyncAction::SetPendingPeer(PendingPeer::new(connection_info.clone(), now.clone()))),
            Action::<U, T, S>::MessageSend(connection_info, message_wrapped),
        ])
    }

    /// Confirm a new peer into our syncing engine (ie, start peering)
    #[tracing::instrument(skip(self), fields(peer = %peer_pubkey))]
    pub fn peer_confirm<U, S>(&self, peer_pubkey: &PeerPubkey, name: &str, now: &DateTime<Utc>) -> Result<Vec<Action<U, T, S>>>
        where U: serde::Serialize,
              T: Clone + serde::Serialize,
              S: serde::Serialize,
    {
        info!("sync.peer_confirm() -- confirm peering with {} // {:?}", name, peer_pubkey);
        let body = Event::<U, T, S>::PeerConfirm {
            name: self.name().clone(),
        };
        let peer_info = self.find_peer(peer_pubkey, None)
            .ok_or(Error::PeerNotFound(peer_pubkey.clone()))?;
        let connection_info = peer_info.connection_info().clone();
        let outgoing_wrapped = self.create_sealed_message(peer_info.peer(), body, vec![], now)?;
        let mut peer_mod = peer_info.clone();
        peer_mod.peer_mut().set_name(name.to_string());
        peer_mod.set_active(true);
        peer_mod.set_last_ping(now.clone());
        Ok(vec![
            Action::Sync(SyncAction::SetPeer(peer_mod)),
            Action::MessageSend(connection_info, outgoing_wrapped),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        peer::{self, ConfirmationMode, SubscriptionConfig},
        util,
    };
    use sodiumoxide::{
        crypto::box_::curve25519xsalsa20poly1305,
    };

    fn make_sync<T>(name: &str) -> Sync<T> {
        Sync::<T>::new(name.into(), Keypair::new_random(), PeeringConfig::new(
            ConfirmationMode::Private { whitelist: vec![] },
            SubscriptionConfig::Blacklist(vec![])
        ))
    }

    fn sync_to_peer<T>(sync: &Sync<T>, our_seckey: &curve25519xsalsa20poly1305::SecretKey) -> Peer {
        Peer::new(sync.name().into(), sync.keypair().to_pubkey(), our_seckey)
    }

    fn make_message<U, T, S>(body: Event<U, T, S>, topic_graph: Vec<TopicGraphEntry<T>>) -> (Message<U, T, S>, PeerPubkey) {
        let id = message::MessageID::new(util::now(), message::gen_nonce());
        (
            Message::new(id, body, topic_graph),
            Keypair::new_random().to_pubkey(),
        )
    }

    #[test]
    fn find_peer() {
        let mut sync = make_sync::<()>("jasper");
        let mut peers = HashMap::new();
        let peer1 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer2 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer3 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        peers.insert(peer1.pubkey().clone(), PeerInfo::new(peer1.clone(), ConnectionInfo::new("69.69.42.69:42").unwrap(), true, util::now()));
        peers.insert(peer2.pubkey().clone(), PeerInfo::new(peer2.clone(), ConnectionInfo::new("1.1.1.1:69").unwrap(), false, util::now()));
        sync.peers = peers;
        assert!(sync.find_peer(peer1.pubkey(), Some(true)).is_some());
        assert!(sync.find_peer(peer1.pubkey(), Some(false)).is_none());
        assert!(sync.find_peer(peer1.pubkey(), None).is_some());
        assert!(sync.find_peer(peer2.pubkey(), Some(false)).is_some());
        assert!(sync.find_peer(peer2.pubkey(), Some(true)).is_none());
        assert!(sync.find_peer(peer2.pubkey(), None).is_some());
        assert!(sync.find_peer(peer3.pubkey(), Some(true)).is_none());
        assert!(sync.find_peer(peer3.pubkey(), Some(false)).is_none());
        assert!(sync.find_peer(peer3.pubkey(), None).is_none());
    }

    #[test]
    fn find_pending_peer_init() {
        let mut sync = make_sync::<()>("pisssss");
        sync.pending_peer_init = vec![
            PendingPeer::new(ConnectionInfo::new("5.1.3.69:8444").unwrap(), util::now()),
            PendingPeer::new(ConnectionInfo::new("5.2.3.69:8444").unwrap(), util::now()),
            PendingPeer::new(ConnectionInfo::new("6.2.3.69:8444").unwrap(), util::now()),
        ];
        let pending1 = sync.find_pending_peer_init(&ConnectionInfo::new("5.2.3.69:8444").unwrap());
        assert!(pending1.is_some());
        let pending2 = sync.find_pending_peer_init(&ConnectionInfo::new("5.1.3.69:8444").unwrap());
        assert!(pending2.is_some());
        let pending3 = sync.find_pending_peer_init(&ConnectionInfo::new("6.2.3.69:8444").unwrap());
        assert!(pending3.is_some());
        let pending4 = sync.find_pending_peer_init(&ConnectionInfo::new("6.1.3.69:8444").unwrap());
        assert!(pending4.is_none());
    }

    #[test]
    fn create_sealed_message() {
        let sync = make_sync::<()>("came home late night about 12 oclock");
        let peer = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let body = Event::<(), (), ()>::Hello;
        let msg1 = sync.create_sealed_message(&peer, body.clone(), vec![], &util::now()).unwrap();
        let msg2 = sync.create_sealed_message(&peer, body.clone(), vec![], &util::now()).unwrap();
        match (msg1, msg2) {
            (MessageWrapped::Sealed(inner1), MessageWrapped::Sealed(inner2))  => {
                assert_eq!(inner1.pubkey_sender(), &sync.keypair.to_pubkey());
                assert_eq!(inner2.pubkey_sender(), &sync.keypair.to_pubkey());
                assert!(inner1.nonce() != inner2.nonce());
                assert!(inner1.sealed_body() != inner2.sealed_body());
            },
            _ => panic!("bad message created"),
        }
    }

    #[test]
    fn is_peer_subscribed_to_topic() {
        let mut sync = make_sync::<String>("went into the bedroom without a knock");
        let peer1 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer2 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer3 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        sync.subscriptions.insert("peee".into(), vec![
            PeerInfo::new(peer1.clone(), ConnectionInfo::new("69.69.42.69:42").unwrap(), true, util::now()),
            PeerInfo::new(peer2.clone(), ConnectionInfo::new("38.38.42.38:22004").unwrap(), true, util::now()),
        ]);
        sync.subscriptions.insert("poooo".into(), vec![
            PeerInfo::new(peer3.clone(), ConnectionInfo::new("22.38.42.22:1118").unwrap(), false, util::now()),
        ]);
        assert!(!sync.is_peer_subscribed_to_topic(&"assss".into(), &peer1));
        assert!(sync.is_peer_subscribed_to_topic(&"peee".into(), &peer1));
        assert!(!sync.is_peer_subscribed_to_topic(&"assss".into(), &peer2));
        assert!(sync.is_peer_subscribed_to_topic(&"peee".into(), &peer2));
        assert!(!sync.is_peer_subscribed_to_topic(&"assss".into(), &peer3));
        assert!(!sync.is_peer_subscribed_to_topic(&"peee".into(), &peer3));
        assert!(!sync.is_peer_subscribed_to_topic(&"poooo".into(), &peer1));
        assert!(!sync.is_peer_subscribed_to_topic(&"poooo".into(), &peer2));
        assert!(sync.is_peer_subscribed_to_topic(&"poooo".into(), &peer3));
    }

    #[test]
    fn filter_message_by_peer() {
        let mut sync = make_sync::<String>("if grandma only knew");
        let peer1 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer2 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer3 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer4 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        sync.subscriptions.insert("what".into(), vec![
            PeerInfo::new(peer1.clone(), ConnectionInfo::new("69.69.42.69:42").unwrap(), true, util::now()),
            PeerInfo::new(peer2.clone(), ConnectionInfo::new("38.38.42.38:22004").unwrap(), true, util::now()),
        ]);
        sync.subscriptions.insert("my friends".into(), vec![
            PeerInfo::new(peer3.clone(), ConnectionInfo::new("22.38.42.22:1118").unwrap(), false, util::now()),
        ]);
        let message1 = Message::new(
            message::MessageID::new(util::now(), message::gen_nonce()),
            Event::<(), String, ()>::Hello,
            vec![
                TopicGraphEntry::new("what".into(), vec![], 0),
                TopicGraphEntry::new("will".into(), vec![], 0),
            ],
        );
        let message2 = Message::new(
            message::MessageID::new(util::now(), message::gen_nonce()),
            Event::<(), String, ()>::Hello,
            vec![
                TopicGraphEntry::new("my friends".into(), vec![], 0),
                TopicGraphEntry::new("say".into(), vec![], 0),
            ],
        );

        assert!(sync.filter_message_by_peer(&peer1, &message1));
        assert!(sync.filter_message_by_peer(&peer2, &message1));
        assert!(sync.filter_message_by_peer(&peer3, &message2));
        assert!(!sync.filter_message_by_peer(&peer1, &message2));
        assert!(!sync.filter_message_by_peer(&peer2, &message2));
        assert!(!sync.filter_message_by_peer(&peer3, &message1));
        assert!(!sync.filter_message_by_peer(&peer4, &message1));
        assert!(!sync.filter_message_by_peer(&peer4, &message2));
    }

    #[test]
    fn apply_sync_actions() {
        let sync = make_sync::<String>("timmy");
        let peer = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        assert_eq!(sync.peers().len(), 0);
        assert_eq!(sync.pending_peer_init().len(), 0);

        let mut sync2 = sync.clone();
        sync2.apply_sync_actions(vec![
            SyncAction::SetPeer(PeerInfo::new(peer.clone(), ConnectionInfo::new("127.0.0.1:1").unwrap(), true, util::now())),
        ]);
        assert_eq!(sync2.peers().len(), 1);
        assert_eq!(sync2.pending_peer_init().len(), 0);
        assert_eq!(sync2.peers().get(peer.pubkey()).unwrap().peer().pubkey(), peer.pubkey());

        let mut sync3 = sync2.clone();
        sync3.apply_sync_actions(vec![
            SyncAction::SetPeer(PeerInfo::new(peer.clone(), ConnectionInfo::new("127.0.0.1:1").unwrap(), true, util::now())),
        ]);
        assert_eq!(sync3.peers().len(), 1);
        assert_eq!(sync3.pending_peer_init().len(), 0);
        assert_eq!(sync3.peers().get(peer.pubkey()).unwrap().peer().pubkey(), peer.pubkey());

        let mut sync4 = sync3.clone();
        sync4.apply_sync_actions(vec![
            SyncAction::UnsetPeer(peer.pubkey().clone()),
        ]);
        assert_eq!(sync4.peers().len(), 0);
        assert_eq!(sync4.pending_peer_init().len(), 0);

        let mut sync5 = sync4.clone();
        sync5.apply_sync_actions(vec![
            SyncAction::SetPendingPeer(PendingPeer::new(ConnectionInfo::new("69.24.123.234:666").unwrap(), util::now())),
            SyncAction::SetPendingPeer(PendingPeer::new(ConnectionInfo::new("77.77.11.99:322").unwrap(), util::now())),
        ]);
        assert_eq!(sync5.peers().len(), 0);
        assert_eq!(sync5.pending_peer_init().len(), 2);
        assert_eq!(sync5.pending_peer_init()[0].connection_info(), &ConnectionInfo::new("69.24.123.234:666").unwrap());
        assert_eq!(sync5.pending_peer_init()[1].connection_info(), &ConnectionInfo::new("77.77.11.99:322").unwrap());

        let mut sync6 = sync5.clone();
        sync6.apply_sync_actions(vec![
            SyncAction::SetPendingPeer(PendingPeer::new(ConnectionInfo::new("69.24.123.234:666").unwrap(), util::now())),
            SyncAction::SetPendingPeer(PendingPeer::new(ConnectionInfo::new("77.77.11.99:322").unwrap(), util::now()),),
        ]);
        assert_eq!(sync6.peers().len(), 0);
        assert_eq!(sync6.pending_peer_init().len(), 2);
        assert_eq!(sync6.pending_peer_init()[0].connection_info(), &ConnectionInfo::new("69.24.123.234:666").unwrap());
        assert_eq!(sync6.pending_peer_init()[1].connection_info(), &ConnectionInfo::new("77.77.11.99:322").unwrap());

        let mut sync7 = sync6.clone();
        sync7.apply_sync_actions(vec![
            SyncAction::UnsetPendingPeer(ConnectionInfo::new("69.24.123.234:666").unwrap()),
        ]);
        assert_eq!(sync7.peers().len(), 0);
        assert_eq!(sync7.pending_peer_init().len(), 1);
        assert_eq!(sync7.pending_peer_init()[0].connection_info(), &ConnectionInfo::new("77.77.11.99:322").unwrap());

        let mut sync8 = sync7.clone();
        sync8.apply_sync_actions(vec![
            SyncAction::UnsetPendingPeer(ConnectionInfo::new("69.24.123.234:666").unwrap()),
            SyncAction::UnsetPendingPeer(ConnectionInfo::new("77.77.11.99:322").unwrap()),
            SyncAction::UnsetPendingPeer(ConnectionInfo::new("75.77.11.99:322").unwrap()),
            SyncAction::UnsetPendingPeer(ConnectionInfo::new("37.77.11.99:322").unwrap()),
            SyncAction::UnsetPendingPeer(ConnectionInfo::new("77.77.19.99:322").unwrap()),
        ]);
        assert_eq!(sync8.peers().len(), 0);
        assert_eq!(sync8.pending_peer_init().len(), 0);

        let peer9_1 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer9_1_info = PeerInfo::new(peer9_1, ConnectionInfo::new("5.5.5.5:222").unwrap(), true, util::now());
        let peer9_2 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer9_2_info = PeerInfo::new(peer9_2, ConnectionInfo::new("6.3.8.9:111").unwrap(), true, util::now());
        let mut sync9 = sync8.clone();
        assert_eq!(sync9.subscriptions.len(), 0);
        sync9.apply_sync_actions(vec![
            SyncAction::SetSubscription("getajob".into(), peer9_1_info.clone()),
            // added twice, but should be deduped
            SyncAction::SetSubscription("getajob".into(), peer9_1_info.clone()),
            SyncAction::SetSubscription("FINE".into(), peer9_2_info.clone()),
        ]);
        assert_eq!(sync9.subscriptions.len(), 2);
        assert_eq!(sync9.subscriptions.get("getajob").unwrap().len(), 1);
        assert_eq!(sync9.subscriptions.get("FINE").unwrap().len(), 1);

        let mut sync10 = sync9.clone();
        // superfluous, i know...
        assert_eq!(sync10.subscriptions.len(), 2);
        assert_eq!(sync10.subscriptions.get("getajob").unwrap().len(), 1);
        assert_eq!(sync10.subscriptions.get("FINE").unwrap().len(), 1);
        sync10.apply_sync_actions(vec![
            // should remove nothing, peer9_1 isn't subscribed to "FINE"
            SyncAction::UnsetSubscription("FINE".into(), peer9_1_info.clone()),
        ]);
        assert_eq!(sync10.subscriptions.len(), 2);
        assert_eq!(sync10.subscriptions.get("getajob").unwrap().len(), 1);
        assert_eq!(sync10.subscriptions.get("FINE").unwrap().len(), 1);
        sync10.apply_sync_actions(vec![
            SyncAction::UnsetSubscription("FINE".into(), peer9_2_info.clone()),
        ]);
        assert_eq!(sync10.subscriptions.len(), 1);
        assert_eq!(sync10.subscriptions.get("getajob").unwrap().len(), 1);
        assert!(sync10.subscriptions.get("FINE").is_none());
        sync10.apply_sync_actions(vec![
            SyncAction::UnsetSubscription("getajob".into(), peer9_2_info.clone()),
        ]);
        assert_eq!(sync10.subscriptions.len(), 1);
        assert_eq!(sync10.subscriptions.get("getajob").unwrap().len(), 1);
        assert!(sync10.subscriptions.get("FINE").is_none());
        sync10.apply_sync_actions(vec![
            SyncAction::UnsetSubscription("getajob".into(), peer9_1_info.clone()),
        ]);
        assert_eq!(sync10.subscriptions.len(), 0);
        assert!(sync10.subscriptions.get("getajob").is_none());
        assert!(sync10.subscriptions.get("FINE").is_none());
    }

    #[test]
    fn tick() {
        let mut sync = make_sync::<()>("her life is in your hands, dude");
        let peer1 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer2 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer3 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let peer1_info = PeerInfo::new(peer1.clone(), ConnectionInfo::new("127.45.23.99:228").unwrap(), true, "2018-06-06T00:02:04Z".parse().unwrap());
        let peer2_info = PeerInfo::new(peer2.clone(), ConnectionInfo::new("233.102.202.17:9887").unwrap(), false, "2018-06-06T00:03:33Z".parse().unwrap());
        let peer3_info = PeerInfo::new(peer3.clone(), ConnectionInfo::new("12.69.42.69:8008").unwrap(), true, "2018-06-06T00:03:58Z".parse().unwrap());
        let pending1 = PendingPeer::new(ConnectionInfo::new("90.90.90.2:344").unwrap(), "2018-06-06T00:02:22Z".parse().unwrap());
        let pending2 = PendingPeer::new(ConnectionInfo::new("188.11.8.5:64454").unwrap(), "2018-06-06T00:03:58Z".parse().unwrap());
        sync.peers.insert(peer1.pubkey().clone(), peer1_info.clone());
        sync.peers.insert(peer2.pubkey().clone(), peer2_info.clone());
        sync.peers.insert(peer3.pubkey().clone(), peer3_info.clone());
        sync.pending_peer_init.push(pending1.clone());
        sync.pending_peer_init.push(pending2.clone());
        let actions1 = sync.tick::<(), ()>(&"2018-06-06T00:04:01Z".parse().unwrap()).unwrap();
        let actions2 = sync.tick::<(), ()>(&"2018-06-06T00:05:01Z".parse().unwrap()).unwrap();
        let actions3 = sync.tick::<(), ()>(&"2018-06-06T00:07:01Z".parse().unwrap()).unwrap();

        assert_eq!(actions1.len(), 1);
        assert_eq!(actions2.len(), 4);
        assert_eq!(actions3.len(), 5);
        match &actions1[0] {
            Action::MessageSend(conn, ..) => assert_eq!(conn, peer1_info.connection_info()),
            _ => panic!("bad match"),
        }
        assert!(
            actions2.iter()
                .find(|x| match x {
                    Action::MessageSend(conn, ..) => conn == peer1_info.connection_info(),
                    _ => false,
                })
                .is_some()
        );
        assert!(
            actions2.iter()
                .find(|x| match x {
                    Action::MessageSend(conn, ..) => conn == peer2_info.connection_info(),
                    _ => false,
                })
                .is_some()
        );
        assert!(
            actions2.iter()
                .find(|x| match x {
                    Action::MessageSend(conn, ..) => conn == peer3_info.connection_info(),
                    _ => false,
                })
                .is_some()
        );
        match &actions2[3] {
            Action::Sync(SyncAction::UnsetPendingPeer(conn)) => {
                assert_eq!(conn, pending1.connection_info());
            }
            _ => panic!("bad"),
        }

        assert!(
            actions3.iter()
                .find(|x| match x {
                    Action::Sync(SyncAction::UnsetPeer(pubkey)) => pubkey == peer2.pubkey(),
                    _ => false,
                })
                .is_some()
        );
        assert!(
            actions3.iter()
                .find(|x| match x {
                    Action::Sync(SyncAction::UnsetPendingPeer(conn)) => {
                        conn == pending2.connection_info()
                    }
                    _ => false,
                })
                .is_some()
        );
    }

    #[test]
    fn process_init_message() {
        let sync = make_sync::<()>("no ragrets");
        let keypair = Keypair::new_random();
        let wrapped = MessageWrapped::Init(keypair.to_pubkey());
        let conninfo = ConnectionInfo::new("54.228.109.71:3338").unwrap();
        let now = util::now();
        let actions1 = sync.process_init_message::<(), ()>(&wrapped, &conninfo, &now).unwrap();
        match &actions1[0] {
            Action::Sync(SyncAction::SetPeer(peer_info)) => {
                assert_eq!(peer_info.peer().pubkey(), &keypair.to_pubkey());
                assert_eq!(peer_info.connection_info(), &conninfo);
                assert_eq!(peer_info.active(), &false);
                assert_eq!(peer_info.last_ping(), &now);
            }
            _ => panic!("panic"),
        }

        let peer_to = Peer::tmp(&Keypair::new_random().to_pubkey(), sync.keypair().seckey());
        let wrapped_sealed = sync.create_sealed_message(&peer_to, Event::<(), (), ()>::Ping, vec![], &now).unwrap();
        let actions2 = sync.process_init_message::<(), ()>(&wrapped_sealed, &conninfo, &now);
        assert_eq!(actions2.unwrap_err(), Error::MessageNotInit);
    }

    #[test]
    fn unwrap_incoming_message() {
        fn make_message(to: &Sync<()>, body: Event<(), (), ()>, from: Option<&Sync<()>>) -> MessageWrapped {
            let now = util::now();
            let sync_from = from.map(|x| x.clone()).unwrap_or(make_sync::<()>("jabroni"));
            let peer = Peer::new("johnny".into(), to.keypair().to_pubkey(), sync_from.keypair().seckey());
            sync_from.create_sealed_message(&peer, body, vec![], &now).unwrap()
        }
        let sync = make_sync::<()>("johnny");

        let sealed_message = make_message(&sync, Event::Hello, None);
        match sealed_message {
            MessageWrapped::Sealed(sealed) => {
                let message = sync.unwrap_incoming_message::<(), ()>(&sealed).unwrap();
                assert!(matches!(message.body(), &Event::Hello));
            }
            _ => panic!("bad dates"),
        }

        let sealed_message2 = make_message(&sync, Event::PeerInit { name: "flibby".into() }, None);
        match sealed_message2 {
            MessageWrapped::Sealed(sealed) => {
                let message = sync.unwrap_incoming_message::<(), ()>(&sealed).unwrap();
                assert!(matches!(message.body(), &Event::PeerInit { .. }));
            }
            _ => panic!("bad dates"),
        }

        let sealed_message3 = make_message(&sync, Event::Ping, None);
        match sealed_message3 {
            MessageWrapped::Sealed(sealed) => {
                let res = sync.unwrap_incoming_message::<(), ()>(&sealed);
                assert_eq!(res.unwrap_err(), Error::PeerNotFound(sealed.pubkey_sender().clone()));
            }
            _ => panic!("bad dates"),
        }

        let mut sync2 = sync.clone();
        let sync_from = make_sync::<()>("jack");
        let sealed_message4 = make_message(&sync, Event::Ping, Some(&sync_from));
        let from_peer = Peer::new("jack".into(), sync_from.keypair().to_pubkey(), sync2.keypair().seckey());
        let from_peer_info = PeerInfo::new(from_peer, ConnectionInfo::new("66.66.66.66:227").unwrap(), true, util::now());
        sync2.peers.insert(from_peer_info.peer().pubkey().clone(), from_peer_info.clone());
        match sealed_message4 {
            MessageWrapped::Sealed(sealed) => {
                let message = sync2.unwrap_incoming_message::<(), ()>(&sealed).unwrap();
                assert!(matches!(message.body(), Event::Ping));
            }
            _ => panic!("bad dates"),
        }
    }

    #[test]
    fn process_event_hello() {
        let sync1 = make_sync::<()>("i like football");
        let (message1, peer_pubkey1) = make_message::<(), (), ()>(Event::Hello, vec![]);
        let conninfo1 = ConnectionInfo::new("1.34.93.92:555").unwrap();
        let res1 = sync1.process_event_hello(&message1, &peer_pubkey1, &conninfo1, &util::now()).unwrap();
        // no pending peer? NO ACTIONS
        assert_eq!(res1.len(), 0);

        let mut sync2 = sync1.clone();
        sync2.pending_peer_init.push(PendingPeer::new(conninfo1.clone(), util::now()));
        let res2 = sync2.process_event_hello(&message1, &peer_pubkey1, &conninfo1, &util::now()).unwrap();
        assert_eq!(res2.len(), 3);
        match &res2[0] {
            Action::Sync(SyncAction::SetPeer(peer_info)) => {
                assert_eq!(peer_info.peer().pubkey(), &peer_pubkey1);
                assert_eq!(peer_info.connection_info(), &conninfo1);
                assert_eq!(peer_info.active(), &false);
            }
            _ => panic!("bad dates"),
        }
        match &res2[1] {
            Action::Sync(SyncAction::UnsetPendingPeer(conninfo)) => {
                assert_eq!(conninfo, &conninfo1);
            }
            _ => panic!("not organic"),
        }
        match &res2[2] {
            Action::MessageSend(conninfo, MessageWrapped::Sealed(sealed)) => {
                assert_eq!(conninfo, &conninfo1);
                assert_eq!(sealed.pubkey_sender(), &sync2.keypair().to_pubkey());
            }
            _ => panic!("my fungal toenail infection's back"),
        }

        let (message2, _) = make_message::<(), (), ()>(Event::Ping, vec![]);
        let res3 = sync2.process_event_hello(&message2, &peer_pubkey1, &conninfo1, &util::now());
        assert_eq!(res3.unwrap_err(), Error::EventMismatch);
    }

    #[test]
    fn process_event_peer_init() {
        fn make_config(mode: ConfirmationMode) -> PeeringConfig {
            PeeringConfig::new(mode, peer::SubscriptionConfig::Blacklist(vec![]))
        }

        // exclude everything
        fn test_auto_deny<U, T, S>(message1: Message<U, T, S>, peer_pubkey1: PeerPubkey, conninfo1: ConnectionInfo, mode: ConfirmationMode)
            where U: Clone + Debug + serde::Serialize + serde::de::DeserializeOwned,
                  T: Clone + Debug + serde::Serialize + serde::de::DeserializeOwned,
                  S: Clone + Debug + serde::Serialize + serde::de::DeserializeOwned,
        {
            let mut sync1 = make_sync::<T>("it is fun");
            sync1.peering_config = make_config(mode);
            let res1 = sync1.process_event_peer_init(&message1, &peer_pubkey1, &conninfo1, &util::now()).unwrap();
            assert_eq!(res1.len(), 0);
        }

        fn test_for_auto_confirm<U, T, S>(message2: Message<U, T, S>, peer_pubkey2: PeerPubkey, conninfo2: ConnectionInfo, mode: ConfirmationMode)
            where U: Clone + Debug + serde::Serialize + serde::de::DeserializeOwned,
                  T: Clone + Debug + serde::Serialize + serde::de::DeserializeOwned,
                  S: Clone + Debug + serde::Serialize + serde::de::DeserializeOwned,
        {
            let mut sync_me2 = make_sync::<T>("i like football");
            let mut sync2 = make_sync::<T>("i like to run");
            let peer2 = sync_to_peer(&sync_me2, sync2.keypair().seckey());
            let peer_me2 = sync_to_peer(&sync2, sync_me2.keypair().seckey());
            let now2 = util::now();
            let peer_info2 = PeerInfo::new(peer2.clone(), conninfo2.clone(), true, now2.clone());
            let peer_info_me2 = PeerInfo::new(peer_me2.clone(), ConnectionInfo::new("55.66.77.88:123").unwrap(), true, now2.clone());
            sync2.peers.insert(peer_pubkey2.clone(), peer_info2);
            sync_me2.peers.insert(peer_me2.pubkey().clone(), peer_info_me2);
            sync2.peering_config = make_config(mode);
            let now2_2 = util::now();
            let res2 = sync2.process_event_peer_init(&message2, &peer_pubkey2, &conninfo2, &now2_2).unwrap();
            assert_eq!(res2.len(), 2);
            match &res2[0] {
                Action::Sync(SyncAction::SetPeer(peer_info)) => {
                    assert_eq!(peer_info.peer().name(), "beavis");
                    assert_eq!(peer_info.peer().pubkey(), peer2.pubkey());
                    assert_eq!(peer_info.connection_info(), &conninfo2);
                    assert_eq!(peer_info.active(), &true);
                    assert_eq!(peer_info.last_ping(), &now2_2);
                }
                _ => panic!("ohhhhhh geez now dat's a muskie if i've ever seeen one ya know.")
            }
            match &res2[1] {
                Action::MessageSend(conninfo, MessageWrapped::Sealed(sealed)) => {
                    assert_eq!(conninfo, &conninfo2);
                    let unwrapped = sync_me2.unwrap_incoming_message::<U, S>(&sealed).unwrap();
                    assert!(matches!(unwrapped.body(), &Event::PeerConfirm { .. }));
                }
                _ => panic!("i got a curse for you it's called tomorrow morning, your ass is out of here"),
            }
        }

        fn test_for_confirm_action<U, T, S>(message3: Message<U, T, S>, peer_pubkey3: PeerPubkey, conninfo3: ConnectionInfo, mode: ConfirmationMode)
            where U: Clone + PartialEq + Debug + serde::Serialize + serde::de::DeserializeOwned,
                  T: Clone + PartialEq + Debug + serde::Serialize + serde::de::DeserializeOwned,
                  S: Clone + PartialEq + Debug + serde::Serialize + serde::de::DeserializeOwned,
        {
            let mut sync_me3 = make_sync::<T>("i like football");
            let mut sync3 = make_sync::<T>("it makes me glad");
            let peer3 = sync_to_peer(&sync_me3, sync3.keypair().seckey());
            let peer_me3 = sync_to_peer(&sync3, sync_me3.keypair().seckey());
            let now3 = util::now();
            let peer_info3 = PeerInfo::new(peer3.clone(), conninfo3.clone(), true, now3.clone());
            let peer_info_me3 = PeerInfo::new(peer_me3.clone(), ConnectionInfo::new("55.66.77.88:123").unwrap(), true, now3.clone());
            sync3.peers.insert(peer_pubkey3.clone(), peer_info3);
            sync_me3.peers.insert(peer_me3.pubkey().clone(), peer_info_me3);
            sync3.peering_config = make_config(mode);
            let res3 = sync3.process_event_peer_init(&message3, &peer_pubkey3, &conninfo3, &util::now()).unwrap();
            assert_eq!(res3.len(), 1);
            match &res3[0] {
                Action::Confirm(ev) => {
                    assert_eq!(ev, message3.body());
                }
                _ => panic!("ohhhhhh geez now dat's a muskie if i've ever seeen one ya know.")
            }
        }

        let (message1, peer_pubkey1) = make_message::<(), (), ()>(Event::PeerInit { name: "beavis".into() }, vec![]);
        test_auto_deny(
            message1.clone(),
            peer_pubkey1.clone(),
            ConnectionInfo::new("33.213.199.56:418").unwrap(),
            ConfirmationMode::Private { whitelist: vec![] },
        );
        test_auto_deny(
            message1.clone(),
            peer_pubkey1.clone(),
            ConnectionInfo::new("33.213.199.56:418").unwrap(),
            ConfirmationMode::Confirm { whitelist: vec![], blacklist: vec![peer_pubkey1.clone()] },
        );
        test_auto_deny(
            message1.clone(),
            peer_pubkey1.clone(),
            ConnectionInfo::new("33.213.199.56:418").unwrap(),
            ConfirmationMode::PublicAgent { whitelist: vec![], blacklist: vec![peer_pubkey1.clone()] },
        );

        let (message2, peer_pubkey2) = make_message::<(), (), ()>(Event::PeerInit { name: "beavis".into() }, vec![]);
        test_for_auto_confirm(
            message2.clone(),
            peer_pubkey2.clone(),
            ConnectionInfo::new("33.213.199.56:418").unwrap(),
            ConfirmationMode::Private { whitelist: vec![peer_pubkey2.clone()] },
        );
        test_for_auto_confirm(
            message2.clone(),
            peer_pubkey2.clone(),
            ConnectionInfo::new("33.213.199.56:418").unwrap(),
            ConfirmationMode::Confirm { whitelist: vec![peer_pubkey2.clone()], blacklist: vec![] },
        );
        test_for_auto_confirm(
            message2.clone(),
            peer_pubkey2.clone(),
            ConnectionInfo::new("33.213.199.56:418").unwrap(),
            ConfirmationMode::PublicAgent { whitelist: vec![], blacklist: vec![] },
        );

        let (message3, peer_pubkey3) = make_message::<(), (), ()>(Event::PeerInit { name: "beavis".into() }, vec![]);
        test_for_confirm_action(
            message3.clone(),
            peer_pubkey3.clone(),
            ConnectionInfo::new("33.213.199.56:418").unwrap(),
            ConfirmationMode::Confirm { whitelist: vec![], blacklist: vec![] },
        );

        let (message4, peer_pubkey4) = make_message::<(), (), ()>(Event::Ping, vec![]);
        let mut sync4 = make_sync::<()>("i play football");
        sync4.peering_config = make_config(ConfirmationMode::Private { whitelist: vec![] });
        let res4 = sync4.process_event_peer_init(&message4, &peer_pubkey4, &ConnectionInfo::new("66.77.88.121:554").unwrap(), &util::now());
        assert_eq!(res4.unwrap_err(), Error::EventMismatch);
    }

    #[test]
    fn process_event_peer_confirm() {
        // wrong event, should break
        let sync1 = make_sync::<()>("with my dad");
        let (message1, pubkey_sender1) = make_message::<(), (), ()>(Event::Ping, vec![]);
        let res1 = sync1.process_event_peer_confirm(&message1, &pubkey_sender1, &util::now());
        assert_eq!(res1.unwrap_err(), Error::EventMismatch);

        // right event, but no existing peer record, should break
        let sync2 = make_sync::<()>("- a poem by nate");
        let (message2, pubkey_sender2) = make_message::<(), (), ()>(Event::PeerConfirm { name: "jackson".into() }, vec![]);
        let res2 = sync2.process_event_peer_confirm(&message2, &pubkey_sender2, &util::now());
        assert_eq!(res2.unwrap_err(), Error::PeerNotFound(pubkey_sender2));

        // right event, peer added, should work
        let mut sync3 = make_sync::<()>("larry");
        let sync3_from = make_sync::<()>("alright shutup parker");  // thank you, parker. shutup. thank you.
        let (message3, _) = make_message::<(), (), ()>(Event::PeerConfirm { name: "jackson".into() }, vec![]);
        let peer3 = sync_to_peer(&sync3_from, sync3.keypair().seckey());
        let peer3_info = PeerInfo::new(peer3.clone(), ConnectionInfo::new("44.88.14.56:1999").unwrap(), false, util::now());
        sync3.peers.insert(peer3.pubkey().clone(), peer3_info);
        let now3 = util::now(); // THATS WHAT I CALL MUSIC
        let res3 = sync3.process_event_peer_confirm(&message3, peer3.pubkey(), &now3).unwrap();
        assert_eq!(res3.len(), 1);
        match &res3[0] {
            Action::Sync(SyncAction::SetPeer(peer_mod)) => {
                assert_eq!(peer_mod.peer().name(), "jackson");
                assert_eq!(peer_mod.active(), &true);
                assert_eq!(peer_mod.last_ping(), &now3);
            }
            _ => panic!("my goat hurts"),
        }
    }

    #[test]
    fn process_event_ping() {
        // wrong event, should break
        let sync1 = make_sync::<()>("well i you leave enough room for my fist");
        let (message1, pubkey_sender1) = make_message::<(), (), ()>(Event::Pong, vec![]);
        let res1 = sync1.process_event_ping(&message1, &pubkey_sender1, &util::now());
        assert_eq!(res1.unwrap_err(), Error::EventMismatch);

        // right event, but no existing peer record, should break
        let sync2 = make_sync::<()>("because i'm going to ram it into your stomach!");
        let (message2, pubkey_sender2) = make_message::<(), (), ()>(Event::Ping, vec![]);
        let res2 = sync2.process_event_ping(&message2, &pubkey_sender2, &util::now());
        assert_eq!(res2.unwrap_err(), Error::PeerNotFound(pubkey_sender2));

        // right event, peer added, should work
        let mut sync3 = make_sync::<()>("i will, bye.");
        let mut sync3_from = make_sync::<()>("mort, preferred realty");
        let (message3, _) = make_message::<(), (), ()>(Event::Ping, vec![]);
        let conninfo3 = ConnectionInfo::new("44.55.222.111:7001").unwrap();
        let peer3 = sync_to_peer(&sync3_from, sync3.keypair().seckey());
        let peer3_info = PeerInfo::new(peer3.clone(), conninfo3.clone(), false, util::now());
        let peer3_from = sync_to_peer(&sync3, sync3_from.keypair().seckey());
        let peer3_from_info = PeerInfo::new(peer3_from.clone(), ConnectionInfo::new("1.1.1.1:1").unwrap(), true, util::now());
        sync3.peers.insert(peer3.pubkey().clone(), peer3_info.clone());
        sync3_from.peers.insert(peer3_from.pubkey().clone(), peer3_from_info.clone());
        let now3 = util::now(); // THATS WHAT I CALL MUSIC
        let res3 = sync3.process_event_ping(&message3, peer3.pubkey(), &now3).unwrap();
        assert_eq!(res3.len(), 2);
        match &res3[0] {
            Action::Sync(SyncAction::SetPeer(peer_mod)) => {
                assert_eq!(peer_mod.peer().name(), peer3_info.peer().name());
                assert_eq!(peer_mod.active(), peer3_info.active());
                assert_eq!(peer_mod.last_ping(), &now3);
            }
            _ => panic!("my goat hurts"),
        }
        match &res3[1] {
            Action::MessageSend(conninfo, MessageWrapped::Sealed(sealed)) => {
                assert_eq!(conninfo, &conninfo3);
                let opened = sync3_from.unwrap_incoming_message::<(), ()>(&sealed).unwrap();
                assert_eq!(opened.body(), &Event::Pong);
            }
            _ => panic!("gahhhhhh timmy"),
        }
    }

    #[test]
    fn process_event_pong() {
        // wrong event, should break
        let sync1 = make_sync::<()>("well i you leave enough room for my fist");
        let (message1, pubkey_sender1) = make_message::<(), (), ()>(Event::Ping, vec![]);
        let res1 = sync1.process_event_pong(&message1, &pubkey_sender1, &util::now());
        assert_eq!(res1.unwrap_err(), Error::EventMismatch);

        // right event, but no existing peer record, should break
        let sync2 = make_sync::<()>("because i'm going to ram it into your stomach!");
        let (message2, pubkey_sender2) = make_message::<(), (), ()>(Event::Pong, vec![]);
        let res2 = sync2.process_event_pong(&message2, &pubkey_sender2, &util::now());
        assert_eq!(res2.unwrap_err(), Error::PeerNotFound(pubkey_sender2));

        // right event, peer added, should work
        let mut sync3 = make_sync::<()>("i will, bye.");
        let sync3_from = make_sync::<()>("alright shutup parker");  // thank you, parker. shutup. thank you.
        let (message3, _) = make_message::<(), (), ()>(Event::Pong, vec![]);
        let peer3 = sync_to_peer(&sync3_from, sync3.keypair().seckey());
        let peer3_info = PeerInfo::new(peer3.clone(), ConnectionInfo::new("34.71.44.129:2232").unwrap(), false, util::now());
        sync3.peers.insert(peer3.pubkey().clone(), peer3_info.clone());
        let now3 = util::now(); // THATS WHAT I CALL MUSIC
        let res3 = sync3.process_event_pong(&message3, peer3.pubkey(), &now3).unwrap();
        assert_eq!(res3.len(), 1);
        match &res3[0] {
            Action::Sync(SyncAction::SetPeer(peer_mod)) => {
                assert_eq!(peer_mod.peer().name(), peer3_info.peer().name());
                assert_eq!(peer_mod.active(), peer3_info.active());
                assert_eq!(peer_mod.last_ping(), &now3);
            }
            _ => panic!("my goat hurts"),
        }
    }

    #[test]
    fn process_event_query_messages_by_id() {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        enum Todo {
            MakeTable,
            EatPastry,
            GetAJob,
        }
        let messages = vec![
            make_message::<Todo, String, ()>(Event::User(Todo::MakeTable), vec![
                TopicGraphEntry::new("jarko".into(), vec![], 0),
                TopicGraphEntry::new("jiminy".into(), vec![], 0),
            ]).0,
            make_message::<Todo, String, ()>(Event::User(Todo::GetAJob), vec![
                TopicGraphEntry::new("owl".into(), vec![], 0),
                TopicGraphEntry::new("oof".into(), vec![], 0),
            ]).0,
            make_message::<Todo, String, ()>(Event::User(Todo::EatPastry), vec![
                TopicGraphEntry::new("zing".into(), vec![], 0),
                TopicGraphEntry::new("zork".into(), vec![], 0),
                TopicGraphEntry::new("zelebreio".into(), vec![], 0),
            ]).0,
        ];

        // wrong event, but empty messages, should return []
        let sync1 = make_sync::<String>("i'm not a pervert!!");
        let (message1, pubkey_sender1) = make_message::<Todo, String, ()>(Event::Ping, vec![]);
        let res1 = sync1.process_event_query_messages_by_id(&message1, &pubkey_sender1, &vec![]).unwrap();
        assert_eq!(res1.len(), 0);

        // right event, but no existing peer record, but empty messages, should return []
        let sync2 = make_sync::<String>("jordie");
        let (message2, pubkey_sender2) = make_message::<Todo, String, ()>(Event::QueryMessagesByID { ids: vec![] }, vec![]);
        let res2 = sync2.process_event_query_messages_by_id(&message2, &pubkey_sender2, &vec![]).unwrap();
        assert_eq!(res2.len(), 0);

        // wrong event, should break
        let sync3 = make_sync::<String>("i'm not a pervert!!");
        let (message3, pubkey_sender3) = make_message::<Todo, String, ()>(Event::Ping, vec![]);
        let res3 = sync3.process_event_query_messages_by_id(&message3, &pubkey_sender3, &messages);
        assert_eq!(res3.unwrap_err(), Error::EventMismatch);

        // right event, but no existing peer record, should break
        let sync4 = make_sync::<String>("jordie");
        let (message4, pubkey_sender4) = make_message::<Todo, String, ()>(Event::QueryMessagesByID { ids: vec![] }, vec![]);
        let res4 = sync4.process_event_query_messages_by_id(&message4, &pubkey_sender4, &messages);
        assert_eq!(res4.unwrap_err(), Error::PeerNotFound(pubkey_sender4));

        let mut sync5 = make_sync::<String>("fara");
        let mut sync5_from = make_sync::<String>("scooter");
        let peer5 = sync_to_peer(&sync5_from, sync5.keypair().seckey());
        let peer5_from = sync_to_peer(&sync5, sync5_from.keypair().seckey());
        let (message5, _) = make_message::<Todo, String, ()>(Event::QueryMessagesByID { ids: vec![] }, vec![]);
        let conninfo5 = ConnectionInfo::new("123.22.94.125:9992").unwrap();
        let peer5_info = PeerInfo::new(peer5.clone(), conninfo5.clone(), true, util::now());
        let peer5_from_info = PeerInfo::new(peer5_from.clone(), ConnectionInfo::new("4.4.4.7:3333").unwrap(), true, util::now());
        sync5.peers.insert(peer5.pubkey().clone(), peer5_info.clone());
        sync5.subscriptions.insert("jiminy".into(), vec![peer5_info.clone()]);
        sync5.subscriptions.insert("jarko".into(), vec![peer5_info.clone()]);
        sync5.subscriptions.insert("owl".into(), vec![peer5_info.clone()]);
        sync5_from.peers.insert(peer5_from.pubkey().clone(), peer5_from_info.clone());
        let res5 = sync5.process_event_query_messages_by_id(&message5, peer5.pubkey(), &messages).unwrap();
        assert_eq!(res5.len(), 2);
        match &res5[0] {
            Action::MessageSend(conninfo, MessageWrapped::Sealed(sealed)) => {
                assert_eq!(conninfo, &conninfo5);
                let opened = sync5_from.unwrap_incoming_message::<Todo, ()>(sealed).unwrap();
                assert_eq!(opened.body(), &Event::User(Todo::MakeTable));
            }
            _ => panic!("gee willickers"),
        }
        match &res5[1] {
            Action::MessageSend(conninfo, MessageWrapped::Sealed(sealed)) => {
                assert_eq!(conninfo, &conninfo5);
                let opened = sync5_from.unwrap_incoming_message::<Todo, ()>(sealed).unwrap();
                assert_eq!(opened.body(), &Event::User(Todo::GetAJob));
            }
            _ => panic!("gee willickers"),
        }
    }

    #[test]
    fn process_event_query_messages_by_depth() {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        enum Todo {
            MakeTable,
            EatPastry,
            GetAJob,
        }
        let messages = vec![
            make_message::<Todo, String, ()>(Event::User(Todo::MakeTable), vec![
                TopicGraphEntry::new("jarko".into(), vec![], 0),
                TopicGraphEntry::new("jiminy".into(), vec![], 0),
            ]).0,
            make_message::<Todo, String, ()>(Event::User(Todo::GetAJob), vec![
                TopicGraphEntry::new("owl".into(), vec![], 0),
                TopicGraphEntry::new("oof".into(), vec![], 0),
            ]).0,
            make_message::<Todo, String, ()>(Event::User(Todo::EatPastry), vec![
                TopicGraphEntry::new("zing".into(), vec![], 0),
                TopicGraphEntry::new("zork".into(), vec![], 0),
                TopicGraphEntry::new("zelebreio".into(), vec![], 0),
            ]).0,
        ];

        // wrong event, but empty messages, should return []
        let sync1 = make_sync::<String>("i'm not a pervert!!");
        let (message1, pubkey_sender1) = make_message::<Todo, String, ()>(Event::Ping, vec![]);
        let res1 = sync1.process_event_query_messages_by_depth(&message1, &pubkey_sender1, &vec![]).unwrap();
        assert_eq!(res1.len(), 0);

        // right event, but no existing peer record, but empty messages, should return []
        let sync2 = make_sync::<String>("jordie");
        let (message2, pubkey_sender2) = make_message::<Todo, String, ()>(Event::QueryMessagesByDepth { topic: "jiminy".into(), depth: 4 }, vec![]);
        let res2 = sync2.process_event_query_messages_by_depth(&message2, &pubkey_sender2, &vec![]).unwrap();
        assert_eq!(res2.len(), 0);

        // wrong event, should break
        let sync3 = make_sync::<String>("i'm not a pervert!!");
        let (message3, pubkey_sender3) = make_message::<Todo, String, ()>(Event::Ping, vec![]);
        let res3 = sync3.process_event_query_messages_by_depth(&message3, &pubkey_sender3, &messages);
        assert_eq!(res3.unwrap_err(), Error::EventMismatch);

        // right event, but no existing peer record, should break
        let sync4 = make_sync::<String>("jordie");
        let (message4, pubkey_sender4) = make_message::<Todo, String, ()>(Event::QueryMessagesByDepth { topic: "jiminy".into(), depth: 4 }, vec![]);
        let res4 = sync4.process_event_query_messages_by_depth(&message4, &pubkey_sender4, &messages);
        assert_eq!(res4.unwrap_err(), Error::PeerNotFound(pubkey_sender4));

        let mut sync5 = make_sync::<String>("fara");
        let mut sync5_from = make_sync::<String>("scooter");
        let peer5 = sync_to_peer(&sync5_from, sync5.keypair().seckey());
        let peer5_from = sync_to_peer(&sync5, sync5_from.keypair().seckey());
        let (message5, _) = make_message::<Todo, String, ()>(Event::QueryMessagesByDepth { topic: "jiminy".into(), depth: 4 }, vec![]);
        let conninfo5 = ConnectionInfo::new("55.66.11.17:556").unwrap();
        let peer5_info = PeerInfo::new(peer5.clone(), conninfo5.clone(), true, util::now());
        let peer5_from_info = PeerInfo::new(peer5_from.clone(), ConnectionInfo::new("4.4.4.7:3333").unwrap(), true, util::now());
        sync5.peers.insert(peer5.pubkey().clone(), peer5_info.clone());
        sync5.subscriptions.insert("jiminy".into(), vec![peer5_info.clone()]);
        sync5.subscriptions.insert("jarko".into(), vec![peer5_info.clone()]);
        sync5.subscriptions.insert("owl".into(), vec![peer5_info.clone()]);
        sync5_from.peers.insert(peer5_from.pubkey().clone(), peer5_from_info.clone());
        let res5 = sync5.process_event_query_messages_by_depth(&message5, peer5.pubkey(), &messages).unwrap();
        assert_eq!(res5.len(), 2);
        match &res5[0] {
            Action::MessageSend(conninfo, MessageWrapped::Sealed(sealed)) => {
                assert_eq!(conninfo, &conninfo5);
                let opened = sync5_from.unwrap_incoming_message::<Todo, ()>(sealed).unwrap();
                assert_eq!(opened.body(), &Event::User(Todo::MakeTable));
            }
            _ => panic!("gee willickers"),
        }
        match &res5[1] {
            Action::MessageSend(conninfo, MessageWrapped::Sealed(sealed)) => {
                assert_eq!(conninfo, &conninfo5);
                let opened = sync5_from.unwrap_incoming_message::<Todo, ()>(sealed).unwrap();
                assert_eq!(opened.body(), &Event::User(Todo::GetAJob));
            }
            _ => panic!("gee willickers"),
        }
    }

    #[test]
    fn process_event_subscribe() {
        #[derive(Clone, Debug, Serialize, Deserialize)]
        struct MySubscription<T> {
            valid: bool,
            topic: T,
        }

        impl<T> Subscription<T> for MySubscription<T> {
            fn verify(&self) -> Option<&T> {
                if self.valid {
                    Some(&self.topic)
                } else {
                    None
                }
            }
        }

        let mut sync1 = make_sync::<String>("getforked");
        let peer1 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync1.keypair().seckey());
        let peer1_info = PeerInfo::new(peer1.clone(), ConnectionInfo::new("44.33.77.123:5556").unwrap(), true, util::now());
        sync1.peers.insert(peer1.pubkey().clone(), peer1_info);
        let body1 = Event::<(), String, MySubscription<String>>::Subscribe(vec![
            MySubscription { valid: false, topic: "getajob".into() },
            MySubscription { valid: true, topic: "poopy butt".into() },
            MySubscription { valid: true, topic: "I HAVE HERPES".into() },
        ]);
        let (message1, _) = make_message(body1, vec![]);
        let res1 = sync1.process_event_subscribe(&message1, &peer1.pubkey()).unwrap();
        // only 2 because the first is invalid LOLOLOL!!! AAHAHAHAHAHAHH OMFGROFLMFAO
        assert_eq!(res1.len(), 2);
        match &res1[0] {
            Action::Sync(SyncAction::SetSubscription(topic, peer_info)) => {
                assert_eq!(topic, "poopy butt");
                assert_eq!(peer_info.peer(), &peer1);
            }
            _ => panic!("ew you wore those shoes??"),
        }
        match &res1[1] {
            Action::Sync(SyncAction::SetSubscription(topic, peer_info)) => {
                assert_eq!(topic, "I HAVE HERPES");
                assert_eq!(peer_info.peer(), &peer1);
            }
            _ => panic!("HODL"),
        }

        // bad dates
        let (message2, pubkey_sender2) = make_message(Event::Ping, vec![]);
        let res2 = sync1.process_event_subscribe::<(), MySubscription<String>>(&message2, &pubkey_sender2);
        assert_eq!(res2.unwrap_err(), Error::EventMismatch);

        // no peer
        let body3 = Event::<(), String, MySubscription<String>>::Subscribe(vec![
            MySubscription { valid: true, topic: "FORENSIC EVIDENCE NEAR THE TORSO".into() },
        ]);
        let (message3, pubkey_sender3) = make_message(body3, vec![]);
        let res3 = sync1.process_event_subscribe::<(), MySubscription<String>>(&message3, &pubkey_sender3);
        assert_eq!(res3.unwrap_err(), Error::PeerNotFound(pubkey_sender3));
    }

    #[test]
    fn process_event_unsubscribe() {
        let mut sync1 = make_sync::<String>("getforked");
        let peer1 = Peer::tmp(&Keypair::new_random().to_pubkey(), sync1.keypair().seckey());
        let peer1_info = PeerInfo::new(peer1.clone(), ConnectionInfo::new("44.33.77.123:5556").unwrap(), true, util::now());
        sync1.peers.insert(peer1.pubkey().clone(), peer1_info);
        let body1 = Event::<(), String, ()>::Unsubscribe(vec![
            "getajob".into(),
            "poopy butt".into(),
            "I HAVE HERPES".into(),
        ]);
        let (message1, _) = make_message(body1, vec![]);
        let res1 = sync1.process_event_unsubscribe(&message1, &peer1.pubkey()).unwrap();
        assert_eq!(res1.len(), 3);
        match &res1[0] {
            Action::Sync(SyncAction::UnsetSubscription(topic, peer_info)) => {
                assert_eq!(topic, "getajob");
                assert_eq!(peer_info.peer(), &peer1);
            }
            _ => panic!("ew you wore those shoes??"),
        }
        match &res1[1] {
            Action::Sync(SyncAction::UnsetSubscription(topic, peer_info)) => {
                assert_eq!(topic, "poopy butt");
                assert_eq!(peer_info.peer(), &peer1);
            }
            _ => panic!("ew you wore those shoes??"),
        }
        match &res1[2] {
            Action::Sync(SyncAction::UnsetSubscription(topic, peer_info)) => {
                assert_eq!(topic, "I HAVE HERPES");
                assert_eq!(peer_info.peer(), &peer1);
            }
            _ => panic!("HODL"),
        }

        // bad dates
        let (message2, pubkey_sender2) = make_message(Event::Ping, vec![]);
        let res2 = sync1.process_event_unsubscribe::<(), ()>(&message2, &pubkey_sender2);
        assert_eq!(res2.unwrap_err(), Error::EventMismatch);

        // no peer
        let body3 = Event::<(), String, ()>::Unsubscribe(vec![
            "FORENSIC EVIDENCE NEAR THE TORSO".into(),
        ]);
        let (message3, pubkey_sender3) = make_message(body3, vec![]);
        let res3 = sync1.process_event_unsubscribe::<(), ()>(&message3, &pubkey_sender3);
        assert_eq!(res3.unwrap_err(), Error::PeerNotFound(pubkey_sender3));
    }

    #[test]
    fn process_event_query_subscriptions() {
        unimplemented!();
    }

    #[test]
    fn process_event_active_subscriptions() {
        unimplemented!();
    }

    #[test]
    fn init_comm() {
        let sync = make_sync::<String>("timmy");
        let actions = sync.init_comm::<(), String>("192.168.0.69:7174", &util::now()).unwrap();
        assert_eq!(actions.len(), 2);
        match &actions[0] {
            Action::Sync(SyncAction::SetPendingPeer(ref pending)) => {
                assert_eq!(pending.connection_info(), &ConnectionInfo::new("192.168.0.69:7174").unwrap());
            }
            _ => panic!("bad action"),
        }
        match &actions[1] {
            Action::MessageSend(ref conninfo, ref message) => {
                assert_eq!(conninfo, &ConnectionInfo::new("192.168.0.69:7174").unwrap());
                match message {
                    MessageWrapped::Init(ref key) => {
                        assert_eq!(key, &sync.keypair().to_pubkey());
                    }
                    _ => panic!("bad message type"),
                }
            }
            _ => panic!("bad action"),
        }
    }

    #[test]
    fn peer_confirm() {
        let peer_from = make_sync::<()>("timmy");
        let peer_to = make_sync::<()>("jimmy");

        assert_eq!(peer_from.peers().len(), 0);
        assert_eq!(peer_to.peers().len(), 0);
        let message = match peer_from.init_comm::<(), String>("192.168.0.222:7174", &util::now()).unwrap()[1] {
            Action::MessageSend(_, ref message) => message.clone(),
            _ => panic!("bad action"),
        };

        assert_eq!(peer_from.peers().len(), 0);
        assert_eq!(peer_to.peers().len(), 0);

        let conninfo = ConnectionInfo::new("191.168.1.2:666").unwrap();
        let actions: Vec<Action<(), (), ()>> = match &message {
            MessageWrapped::Init(..) => {
                peer_to.process_init_message(&message, &conninfo, &util::now()).unwrap()
            }
            _ => panic!("lol"),
        };

        assert_eq!(peer_from.peers().len(), 0);
        assert_eq!(peer_to.peers().len(), 0);
        assert_eq!(actions.len(), 2);

        let peer_obj = match &actions[0] {
            Action::Sync(SyncAction::SetPeer(ref peer_info)) => {
                assert_eq!(peer_info.connection_info(), &ConnectionInfo::new("191.168.1.2:666").unwrap());
                assert_eq!(peer_info.peer().name(), "<tmp>");
                assert_eq!(peer_info.peer().pubkey(), &peer_from.keypair().to_pubkey());
                assert_eq!(peer_info.active(), &false);
                peer_info.peer().clone()
            }
            _ => panic!("bad action"),
        };
        match &actions[1] {
            Action::MessageSend(ref conninfo, ref message) => {
                assert_eq!(conninfo, &ConnectionInfo::new("191.168.1.2:666").unwrap());
                match message {
                    MessageWrapped::Sealed(sealed) => {
                        let opened = sealed.open::<(), (), ()>(&peer_obj).unwrap();
                        assert_eq!(opened.body(), &Event::Hello);
                    }
                    _ => panic!("bad message"),
                }
            }
            _ => panic!("bad action"),
        }
    }
}

