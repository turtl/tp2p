use crate::{
    error::Result,
    peer::PeerSpec,
};
use tokio::{
    self,
    net::{TcpListener, TcpStream},
};

pub async fn listen(spec: &PeerSpec) -> Result<()> {
    let listener = TcpListener::bind(&format!("{}:{}", spec.address, spec.port)).await?;
    loop {
        let (socket, _) = listener.accept().await?;
        tokio::spawn(async move {
            incoming(socket).await;
        });
    }
    Ok(())
}

