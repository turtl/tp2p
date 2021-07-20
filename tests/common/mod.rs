use async_std::{
    net::{TcpListener, TcpStream},
    task,
};
use futures::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    ops::Deref,
};
use tp2p::{
    ConnectionInfo,
    message::MessageWrapped,
};
use yamux;

pub fn serialize<T: Serialize>(obj: &T) -> Result<Vec<u8>, String> {
    let ser = rmp_serde::to_vec(obj)
        .map_err(|err| format!("ser::ser -- {:?}", err))?;
    Ok(ser)
}

pub fn deserialize<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let obj = rmp_serde::from_read(bytes)
        .map_err(|err| format!("ser::des -- {:?}", err))?;
    Ok(obj)
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionManager {
    connections: HashMap<ConnectionInfo, yamux::Stream>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Default::default()
    }

    pub async fn bind(&mut self, addr: &str, port: u16) -> Result<(), String> {
        let addr = addr.to_string();
        let listener = TcpListener::bind(&format!("{}:{}", addr, port)).await
            .map_err(|err| format!("ConnectionManager::bind() -- couldn't bind listener: {:?}", err))?;

        loop {
            let (mut stream, who) = listener.accept().await
                .map_err(|err| format!("ConnectionManager::bind() -- accept error: {:?}", err))?;
            let from = ConnectionInfo::from(who);
            println!("- incoming: {:?}", who);
            let mut conn = yamux::Connection::new(stream, yamux::Config::default(), yamux::Mode::Server);
            while let stream_maybe = conn.next_stream().await.map_err(|err| format!("ConnectionManager::bind() -- couldn't grab stream: {:?}", err))? {
                let mut stream = stream_maybe.ok_or(format!("ConnectionManager::bind() -- stream is None"))?;
                self.connections.insert(from.clone(), stream);
                //let stream = self.connections.get_mut(&from).ok_or(format!("ConnectionManager::bind() -- stream not found"))?;
                //task::spawn(async move {
                    //while let Some(val) = stream.next().await {
                        //let packet = val.expect("error generating packet");
                        //let msg: MessageWrapped = deserialize(packet.as_ref()).expect("error deserializing stupid message");
                        //println!("** inc: val: {:?}", msg);
                    //}
                //});
            }
        }
        Ok(())
    }

    pub async fn send(&mut self, to: &ConnectionInfo, message: &MessageWrapped) -> Result<(), String> {
        let stream = match self.connections.get_mut(to) {
            Some(val) => val,
            None => {
                let stream = TcpStream::connect(to.deref()).await
                    .map_err(|err| format!("send -- {:?}", err))?;
                let conn = yamux::Connection::new(stream.clone(), yamux::Config::default(), yamux::Mode::Client);
                let mut ctrl = conn.control();
                task::spawn(yamux::into_stream(conn).for_each(|_| future::ready(())));
                let stream = ctrl.open_stream().await
                    .map_err(|err| format!("send -- cannot open yamux stream: {:?}", err))?;
                self.connections.insert(to.clone(), stream);
                self.connections.get_mut(to)
                    .ok_or(format!("send -- cannot find connection {:?}", to))?
            }
        };
        let ser = serialize(message)?;
        stream.write_all(ser.as_slice()).await
            .map_err(|err| format!("send -- problem sending msg: {:?}", err))?;
        println!("strea3: wrote: {:?} -- {:?}", stream, ser.len());    // DEBUG: adf
        Ok(())
    }
}


