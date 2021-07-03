pub mod error;
pub mod comm;
pub mod message;
pub mod peer;
pub mod state;
mod ser;

pub async fn start(peers: &Vec<peer::PeerSpec>) {
    comm::listen(
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_derive::{Serialize, Deserialize};
    use std::time::Duration;
    use tokio;

    #[test]
    fn future_channel() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (mut send, mut recv) = futures::channel::mpsc::channel::<i32>(16);
            tokio::spawn(async move {
                send.try_send(56).unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                send.try_send(42).unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                send.try_send(69).unwrap();
            });
            println!("- fut: rx: {:?}", recv.next().await);
            println!("- fut: rx: {:?}", recv.next().await);
            println!("- fut: rx: {:?}", recv.next().await);
            println!("- fut: rx: {:?}", recv.next().await);
        });
    }

    #[test]
    fn ser() {
        #[derive(Debug, Serialize, Deserialize)]
        struct Person {
            name: String,
            age: u8,
        }

        let me = Person { name: String::from("andrew"), age: 34 };
        let bytes = ser::serialize(&me).unwrap();
        println!("- bytes -- {:x?}", bytes);
        println!("- str   -- {}", String::from_utf8_lossy(&bytes));

        let me2: Person = ser::deserialize(&bytes[..]).unwrap();
        println!("- me    -- {:?}", me2);
    }
}

