use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};

use anyhow::Context;
use tokio::{sync::mpsc, time::timeout};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use upnp::{
    internet_gateway::{InternetGatewayClient, PortMappingProtocol},
    service_client::ScpdClient,
};

use crate::{peers::Peer, utils};

const PEER_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const PORT_RENEW_INTERVAL: Duration = Duration::from_secs(1800);

#[derive(Debug)]
pub enum NewPeer {
    ListenerOrigin(Peer),
}

#[derive(Debug)]
pub struct PeerListener {
    new_torrent_channel: mpsc::Sender<([u8; 20], mpsc::Sender<NewPeer>)>,
}

impl PeerListener {
    pub async fn spawn(port: u16) -> anyhow::Result<Self> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        let listener = utils::bind_tcp_listener(addr).await?;
        let (tx, mut rx) = mpsc::channel(100);
        tokio::spawn(async move {
            let mut map: HashMap<[u8; 20], mpsc::Sender<NewPeer>> = HashMap::new();
            loop {
                tokio::select! {
                    Ok((socket, ip)) = listener.accept() => {
                        tracing::trace!(%ip, "Accepted external connection");
                        match timeout(PEER_CONNECT_TIMEOUT, Peer::new_without_info_hash(socket)).await {
                            Ok(Ok(peer)) => {
                                let info_hash = peer.handshake.info_hash();
                                if let Some(channel) = map.get_mut(&info_hash) {
                                    tracing::debug!("Peer connected via listener {}", ip);
                                    if channel.send(NewPeer::ListenerOrigin(peer)).await.is_err() {
                                        tracing::warn!("Peer connected to outdated torrent");
                                        map.remove(&info_hash);
                                    };
                                } else {
                                    tracing::warn!("Peer {ip} connected but torrent does not exist", );
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::warn!("Failed to construct handshake with peer: {}", e);
                            }
                            Err(_) => {
                                tracing::trace!("Peer with ip {} timed out", ip);
                            }
                        }

                    },
                    Some((info_hash, sender)) = rx.recv() => {
                        map.insert(info_hash, sender);
                    },
                    else => { break; }
                };
            }
            tracing::debug!("Closed peer listener");
        });
        Ok(Self {
            new_torrent_channel: tx,
        })
    }

    pub async fn spawn_with_upnp(
        port: u16,
        client: ScpdClient<InternetGatewayClient>,
        tracker: &TaskTracker,
        cancellation_token: CancellationToken,
    ) -> anyhow::Result<Self> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        let listener = utils::bind_tcp_listener(addr).await?;
        let mut renew_interval =
            tokio::time::interval(PORT_RENEW_INTERVAL + Duration::from_secs(5));
        let mut port_manager = UpnpPortManager::new(port, client).await;
        match &port_manager {
            Ok(_) => tracing::info!("Initiated UPnP port manager"),
            Err(e) => tracing::warn!("Failed to initiate UPnP port manager: {e}"),
        };
        let (tx, mut rx) = mpsc::channel(100);
        tracker.spawn(async move {
            let mut map: HashMap<[u8; 20], mpsc::Sender<NewPeer>> = HashMap::new();
            loop {
                tokio::select! {
                    Ok((socket, ip)) = listener.accept() => {
                        tracing::trace!(%ip, "Accepted external connection");
                        match timeout(PEER_CONNECT_TIMEOUT, Peer::new_without_info_hash(socket)).await {
                            Ok(Ok(peer)) => {
                                let info_hash = peer.handshake.info_hash();
                                if let Some(channel) = map.get_mut(&info_hash) {
                                    tracing::debug!("Peer connected via listener {}", ip);
                                    if channel.send(NewPeer::ListenerOrigin(peer)).await.is_err() {
                                        tracing::warn!("Peer connected to outdated torrent");
                                        map.remove(&info_hash);
                                    };
                                } else {
                                    tracing::warn!("Peer {ip} connected but torrent does not exist", );
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::warn!("Failed to construct handshake with peer: {}", e);
                            }
                            Err(_) => {
                                tracing::trace!("Peer with ip {} timed out", ip);
                            }
                        }

                    },
                    Some((info_hash, sender)) = rx.recv() => {
                        map.insert(info_hash, sender);
                    },
                    _ = cancellation_token.cancelled() => {
                        if let Ok(port_manager) = &mut port_manager {
                            if let Err(e) = port_manager.delete_mapping().await {
                                tracing::error!("Failed to cleanup port mapping: {e}");
                            };
                        }
                        break;
                    }
                    _ = renew_interval.tick() => {
                            if let Ok(port_manager) = &mut port_manager {
                                match port_manager.renew().await {
                                    Ok(_) => tracing::info!("Renewed the port mapping for the next {} seconds", PORT_RENEW_INTERVAL.as_secs()),
                                    Err(e) => tracing::error!("Failed to renew the port mapping: {e}"),
                                };
                            }
                        }
                };
            }
            tracing::debug!("Closed peer listener");
        });
        Ok(Self {
            new_torrent_channel: tx,
        })
    }

    pub async fn subscribe(&self, info_hash: [u8; 20], sender: mpsc::Sender<NewPeer>) {
        self.new_torrent_channel
            .send((info_hash, sender))
            .await
            .unwrap();
    }
}

async fn resolve_local_addr() -> anyhow::Result<SocketAddrV4> {
    use tokio::net::UdpSocket;
    let google = Ipv4Addr::new(8, 8, 8, 8);
    // NOTE: this feels wrong. Find the better solution
    let socket =
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).await?;
    socket
        .connect(SocketAddr::V4(SocketAddrV4::new(google, 0)))
        .await?;
    socket
        .local_addr()
        .context("get local addr")
        .and_then(|addr| match addr {
            SocketAddr::V4(v4) => Ok(v4),
            SocketAddr::V6(_) => Err(anyhow::anyhow!("ipv6 is not expected")),
        })
}

#[derive(Debug)]
struct UpnpPortManager {
    local_addr: SocketAddrV4,
    client: ScpdClient<InternetGatewayClient>,
    any_port_supported: bool,
}

// NOTE: add UDP mapping after implementing utp
impl UpnpPortManager {
    pub async fn new(port: u16, client: ScpdClient<InternetGatewayClient>) -> anyhow::Result<Self> {
        let any_port_supported = client.is_supported("AddAnyPortMapping");
        if !any_port_supported && !client.is_supported("AddPortMapping") {
            return Err(anyhow::anyhow!("port mapping actions are not supported"));
        }

        let mut local_addr = resolve_local_addr().await?;
        local_addr.set_port(port);

        Ok(Self {
            local_addr,
            client,
            any_port_supported,
        })
    }

    pub async fn renew(&mut self) -> anyhow::Result<Option<u16>> {
        const MAPPING_DESCRIPTION: &str = "Media Server Torrent";
        if self.any_port_supported {
            let new_port = self
                .client
                .add_any_port_mapping(
                    None,
                    self.local_addr.port(),
                    PortMappingProtocol::TCP,
                    self.local_addr.port(),
                    *self.local_addr.ip(),
                    MAPPING_DESCRIPTION.to_owned(),
                    PORT_RENEW_INTERVAL.as_secs() as u32,
                )
                .await?;
            Ok(Some(new_port))
        } else {
            self.client
                .add_port_mapping(
                    None,
                    self.local_addr.port(),
                    PortMappingProtocol::TCP,
                    self.local_addr.port(),
                    *self.local_addr.ip(),
                    MAPPING_DESCRIPTION.to_owned(),
                    PORT_RENEW_INTERVAL.as_secs() as u32,
                )
                .await?;
            Ok(None)
        }
    }

    pub async fn delete_mapping(&self) -> anyhow::Result<()> {
        self.client
            .delete_port_mapping(PortMappingProtocol::TCP, self.local_addr.port())
            .await?;
        tracing::info!("Succsessfully cleaned up port mapping");
        Ok(())
    }
}
