use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::connection::PrimaryConnection;
use crate::dock::DockState;

pub struct PrimaryPortServer {
    bind_addr: SocketAddr,
    state: DockState,
}

impl PrimaryPortServer {
    pub fn new(bind_addr: SocketAddr, state: DockState) -> Self {
        Self { bind_addr, state }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        info!(addr = %self.bind_addr, "Primary Dock TCP server listening");

        loop {
            match listener.accept().await {
                Ok((socket, peer_addr)) => {
                    let usb_dev = self.state.first_usb_device().await;
                    let connection =
                        PrimaryConnection::new(socket, peer_addr, self.state.clone(), usb_dev);
                    tokio::spawn(async move {
                        connection.run().await;
                    });
                }
                Err(e) => {
                    error!("Error accepting TCP connection: {}", e);
                }
            }
        }
    }
}
