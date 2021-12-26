use async_std::{
    channel::{self, Receiver, Sender},
    net::{TcpListener, TcpStream},
    task,
};
use bytes::{Bytes, BytesMut, BufMut};
use futures::{
    io::{ReadHalf, WriteHalf},
    prelude::*,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    fmt::Debug,
    ops::Deref,
};
use tp2p::{
    action::Action,
    sync::{ConnectionInfo, Sync},
};

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

pub async fn action_dispatcher<U, T, S>(sync: &mut Sync<T>, tx: &Sender<PeerEvent>, action: Action<U, T, S>) -> Result<(), String>
    where U: Debug,
          T: Debug + Eq + std::hash::Hash,
          S: Debug,
{
    if let Action::Sync(sync_action) = action {
        sync.apply_sync_actions(vec![sync_action]);
    } else {
        match action {
            Action::MessageSend(ref conninfo, ref msg) => {
                tx.send(PeerEvent::Send(conninfo.clone(), serialize(msg)?)).await
                    .map_err(|err| format!("action_dispatcher::MessageSend -- {:?}", err))?;
            }
            Action::Confirm(ref event) => {
                println!("Confirm???? {:?}", event);
            }
            Action::Sync(_) => {}
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct FramedConnection {
    stream: ReadHalf<TcpStream>,
    buffer: BytesMut,
}

impl FramedConnection {
    pub fn new(stream: ReadHalf<TcpStream>) -> Self {
        Self {
            stream,
            buffer: BytesMut::with_capacity(4096),
        }
    }

    fn frame_decode(&mut self) -> Vec<Bytes> {
        let bytes = &mut self.buffer;
        let mut res = Vec::new();
        loop {
            if bytes.len() < 4 {
                return res;
            }
            let len: u32 = ((bytes[0] as u32) << 24) + ((bytes[1] as u32) << 16) + ((bytes[2] as u32) << 8) + (bytes[3] as u32);
            if bytes.len() >= (len + 4) as usize {
                res.push(bytes.split_to((len + 4) as usize).freeze().split_off(4));
            } else {
                return res;
            }
        }
    }

    pub async fn poll_frames(&mut self) -> Result<Vec<Bytes>, String> {
        let mut bytes = [0u8; 4096];
        let n = self.stream.read(&mut bytes).await
            .map_err(|err| format!("PeerManager::bind() -- couldn't read stream: {:?}", err))?;
        if n == 0 {
            // connection closed
            Err(format!("connection closed"))?;
        }
        self.buffer.extend_from_slice(&bytes[0..n]);
        let frames = self.frame_decode();
        Ok(frames)
    }

    pub fn create_framed_message(body: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + body.len());
        buf.put_u32(body.len() as u32);
        buf.put(body);
        buf
    }
}

#[derive(Debug)]
pub enum MgrEvent {
    IncomingConnection(ConnectionInfo, TcpStream),
}

#[derive(Debug)]
pub enum PeerEvent {
    /// internal, can be ignored by everyone but us
    Mgr(MgrEvent),
    /// instruct the peer manager to send a message
    Send(ConnectionInfo, Vec<u8>),
    /// We got an incoming connection. Jus thought u shuld kno...
    Connect(ConnectionInfo),
    /// Peer disconnected. Never liked them anyway.
    Disconnect(ConnectionInfo),
    /// the peer has received an incoming message
    Recv(ConnectionInfo, Bytes),
    /// OK TURN IT OFF!! TURN IT OFF!!
    Quit,
}

#[derive(Debug)]
pub(crate) struct PeerManager {
    pub event_tx: Sender<PeerEvent>,
    event_rx: Receiver<PeerEvent>,
    app_tx: Sender<PeerEvent>,
    pub app_rx: Receiver<PeerEvent>,
}

impl PeerManager {
    pub fn new() -> (Self, Sender<PeerEvent>, Receiver<PeerEvent>) {
        let (event_tx, event_rx) = channel::bounded(64);
        let (app_tx, app_rx) = channel::bounded(64);
        let mgr = Self {
            event_tx,
            event_rx,
            app_tx,
            app_rx,
        };
        let tx = mgr.event_tx.clone();
        let rx = mgr.app_rx.clone();
        (mgr, tx, rx)
    }

    pub async fn start(&self, addr: &str, port: u16) -> Result<(), String> {
        let tx_app = self.app_tx.clone();
        let rx_ev = self.event_rx.clone();
        let event_task = task::spawn(async move { Self::event_handler(tx_app, rx_ev).await });
        self.bind(addr, port).await?;
        event_task.await?;
        Ok(())
    }

    pub async fn event_handler(tx_app: Sender<PeerEvent>, rx_event: Receiver<PeerEvent>) -> Result<(), String> {
        let mut writers: HashMap<ConnectionInfo, WriteHalf<TcpStream>> = HashMap::new();
        let reader = |who: ConnectionInfo, reader: ReadHalf<TcpStream>| {
            let tx_app_reader = tx_app.clone();
            task::spawn(async move {
                let mut framed = FramedConnection::new(reader);
                loop {
                    match framed.poll_frames().await {
                        Ok(frames) => {
                            for frame in frames {
                                tx_app_reader.send(PeerEvent::Recv(who.clone(), frame)).await
                                    .map_err(|err| format!("reader -- {:?}", err))?;
                            }
                        }
                        Err(err) => {
                            if &err == "connection closed" {
                                tx_app_reader.send(PeerEvent::Disconnect(who.clone())).await
                                    .map_err(|err| format!("reader -- {:?}", err))?;
                                break;
                            }
                            Err(err)?;
                        }
                    }
                }
                let res: Result<(), String> = Ok(());
                res
            })
        };
        loop {
            let ev = rx_event.recv().await
                .map_err(|err| format!("PeerManager::event_handler() -- recv error: {:?}", err))?;
            match ev {
                PeerEvent::Mgr(MgrEvent::IncomingConnection(conninfo, stream)) => {
                    let (read, write) = stream.split();
                    reader(conninfo.clone(), read);
                    tx_app.send(PeerEvent::Connect(conninfo.clone())).await
                        .map_err(|err| format!("inc -- {:?}", err))?;
                    writers.insert(conninfo, write);
                }
                PeerEvent::Send(to, message_bytes) => {
                    let stream = match writers.get_mut(&to) {
                        Some(val) => val,
                        None => {
                            let stream = TcpStream::connect(to.deref()).await
                                .map_err(|err| format!("send -- {:?}", err))?;
                            let (read, write) = stream.split();
                            reader(to.clone(), read);
                            writers.insert(to.clone(), write);
                            writers.get_mut(&to)
                                .ok_or(format!("send -- cannot find connection {:?}", to))?
                        }
                    };
                    let framed_message = FramedConnection::create_framed_message(message_bytes.as_slice());
                    stream.write_all(framed_message.as_slice()).await
                        .map_err(|err| format!("send -- problem sending msg: {:?}", err))?;
                }
                PeerEvent::Connect(..) => {}
                PeerEvent::Disconnect(..) => {}
                PeerEvent::Recv(..) => {}
                PeerEvent::Quit => break,
            }
        }
        Ok(())
    }

    pub async fn bind(&self, addr: &str, port: u16) -> Result<(), String> {
        let addr = addr.to_string();
        let listener = TcpListener::bind(&format!("{}:{}", addr, port)).await
            .map_err(|err| format!("PeerManager::bind() -- couldn't bind listener: {:?}", err))?;
        loop {
            let (stream, who) = listener.accept().await
                .map_err(|err| format!("PeerManager::bind() -- accept error: {:?}", err))?;
            let from = ConnectionInfo::from(who);
            self.event_tx.send(PeerEvent::Mgr(MgrEvent::IncomingConnection(from, stream))).await
                .map_err(|err| format!("bind -- {:?}", err))?;
        }
        Ok(())
    }
}

