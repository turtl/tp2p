/// Implement this trait to allow peers to subscribe to topics on your node.
pub trait Subscription<T> {
    /// Verify that this subscription is valid. If it's valid, it returns
    /// `Some(T)` (the topic this subscription is for). `None` means the
    /// verification failed.
    ///
    /// Generally this means checking some kind of cryptographic proof. For
    /// instance, if you're subscribing to events for the identity at pubkey
    /// 1234 then you would post a message saying "hai i am 1234" then sign it
    /// with 1234's private key.
    ///
    /// Or you can always return the topics here if you're working with trusted
    /// peers.
    fn verify(&self) -> Option<T>;
}

