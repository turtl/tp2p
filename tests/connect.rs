mod common;

use async_std::{
    channel,
    task,
};
use common::ConnectionManager;
use futures::prelude::*;
use tp2p::{
    ConnectionInfo, Mode, Sync,
    action::Action,
    message::MessageWrapped,
    peer::Keypair,
};
use yamux;

#[async_std::test]
async fn peer_connect() -> Result<(), String> {
    let mut peer1 = ConnectionManager::new();
    let peer1_task = task::spawn(async move {
        peer1.bind("127.0.0.1", 50020).await.expect("problem binding server");
    });

    let keypair2 = Keypair::new_random();
    let mut peer2 = ConnectionManager::new();
    let mut sync2 = Sync::new(Mode::Agent, "frank".into(), keypair2);
    let actions = sync2.peer_init("127.0.0.1:50020").expect("peer_init failed");
    match &actions[0] {
        &Action::MessageSend(ref info, ref msg) => {
            peer2.send(info, msg).await?;
        }
        _ => panic!("bad action {:?}", actions),
    }
    match &actions[0] {
        &Action::MessageSend(ref info, ref msg) => {
            peer2.send(info, msg).await?;
        }
        _ => panic!("bad action {:?}", actions),
    }

    peer1_task.await;

    Ok(())
}

