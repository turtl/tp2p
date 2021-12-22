/*
mod common;

use async_std::{
    channel,
    task,
};
use common::{
    action_dispatcher,
    deserialize,
    PeerEvent,
    PeerManager,
};
use tp2p::{
    Sync,
    message::MessageWrapped,
    peer::{Keypair, PeeringConfig, ConfirmationMode, SubscriptionConfig},
};

macro_rules! main_loop {
    ($sync:expr, $peer:expr, $tx:expr, $rx:expr, $name:expr) => {
        loop {
            match $rx.try_recv() {
                Ok(PeerEvent::Connect(from)) => {
                    println!(concat!($name, ": new connection! {:?}"), from);
                }
                Ok(PeerEvent::Recv(from, message_bytes)) => {
                    let msg: MessageWrapped = deserialize(message_bytes.as_ref())?;
                    //println!(concat!($name, ": recv: {:?} -- {:?}"), from, msg);  // DEBUG: remove
                    let actions = $sync.process_incoming_message(&msg, from)
                        .map_err(|err| format!("process: {:?}", err))?;
                    for action in actions {
                        println!(concat!($name, ": action -- {:?}"), action);
                        action_dispatcher(&mut $sync, &$tx, action).await?;
                    }
                }
                Err(channel::TryRecvError::Closed) => {
                    break;
                }
                _ => {}
            }
            task::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

#[async_std::test]
async fn peer_connect() -> Result<(), String> {
    env_logger::init();
    let peer1_task = task::spawn(async move {
        let keypair = Keypair::new_random();
        let peering_config = PeeringConfig::new(
            ConfirmationMode::PublicAgent {
                whitelist: vec![],
                blacklist: vec![],
            },
            SubscriptionConfig::Blacklist(vec![]),
        );
        let mut sync = Sync::new("uno".into(), keypair, peering_config);
        let (peer, tx, rx) = PeerManager::new();
        let peer_task = task::spawn(async move {
            peer.start("127.0.0.1", 50020).await.expect("error running peer");
        });
        main_loop! { sync , peer, tx, rx, "peer1" }

        peer_task.await;
        let res: Result<(), String> = Ok(());
        res
    });

    let peer2_task = task::spawn(async move {
        task::sleep(std::time::Duration::from_millis(50)).await;
        let keypair = Keypair::new_random();
        let peering_config = PeeringConfig::new(
            ConfirmationMode::PublicAgent {
                whitelist: vec![],
                blacklist: vec![],
            },
            SubscriptionConfig::Blacklist(vec![]),
        );
        let mut sync = Sync::new("twofer".into(), keypair, peering_config);
        let (peer, tx, rx) = PeerManager::new();
        let peer_task = task::spawn(async move {
            peer.start("127.0.0.1", 50021).await.expect("error running peer");
        });

        let actions = sync.init_comm("127.0.0.1:50020").expect("peer_init failed");
        for action in actions {
            println!("peer2: action -- {:?}", action);
            action_dispatcher(&mut sync, &tx, action).await?;
        }
        main_loop! { sync , peer, tx, rx, "peer2" }

        peer_task.await;
        let res: Result<(), String> = Ok(());
        res
    });

    let res = futures::try_join!(peer1_task, peer2_task);
    assert_eq!(res, Ok(((), ())));

    Ok(())
}
*/

