use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

use anyhow::Context;
use tokio::net::{TcpListener, UdpSocket};
use upnp::{internet_gateway::InternetGatewayClient, search_client, service_client::ScpdClient};

pub fn verify_iter_sha1(hash: &[u8; 20], input: impl Iterator<Item = impl AsRef<[u8]>>) -> bool {
    use sha1::{Digest, Sha1};
    let mut hasher = <Sha1 as sha1::Digest>::new();
    for el in input {
        hasher.update(el);
    }
    let result: [u8; 20] = hasher.finalize().into();
    *hash == result
}

pub fn piece_size(piece_i: usize, piece_length: u32, total_length: u64) -> u64 {
    let piece_length = piece_length as u64;
    let total_pieces = total_length.div_ceil(piece_length);

    if piece_i == total_pieces as usize - 1 {
        let md = total_length % piece_length;
        if md == 0 { piece_length } else { md }
    } else {
        piece_length
    }
}

/// Create tcp listener with n + 1 port if it is taken
pub async fn bind_tcp_listener(mut addr: SocketAddrV4) -> anyhow::Result<TcpListener> {
    loop {
        match TcpListener::bind(addr).await {
            Ok(res) => {
                break Ok(res);
            }
            Err(e) => match e.kind() {
                std::io::ErrorKind::AddrInUse => {
                    let port = addr.port();
                    tracing::warn!("Port {} is taken, trying port {}", port, port + 1);
                    addr.set_port(port + 1);
                }
                _ => return Err(e.into()),
            },
        }
    }
}

/// Create udp socket with n + 1 port if it is taken
pub async fn bind_udp_socket(mut addr: SocketAddrV4) -> anyhow::Result<UdpSocket> {
    loop {
        match UdpSocket::bind(addr).await {
            Ok(res) => {
                break Ok(res);
            }
            Err(e) => match e.kind() {
                std::io::ErrorKind::AddrInUse => {
                    let port = addr.port();
                    tracing::warn!("Port {} is taken, trying port {}", port, port + 1);
                    addr.set_port(port + 1);
                }
                _ => return Err(e.into()),
            },
        }
    }
}

const RESOLVE_IP_TIMEOUT: Duration = Duration::from_millis(400);

/// Fetch client's external ip
pub async fn external_ip(
    upnp_client: Option<&ScpdClient<InternetGatewayClient>>,
) -> anyhow::Result<Ipv4Addr> {
    match ipfy_ip(upnp_client.map_or_else(reqwest::Client::new, |c| c.fetch_client.clone())).await {
        Ok(addr) => {
            tracing::info!(ip = %addr, "Resolved external ip addr using ipfy service");
            return Ok(addr);
        }
        Err(e) => tracing::warn!("Failed to resolve external ip using ipfy: {e}"),
    };
    if let Some(client) = upnp_client {
        match tokio::time::timeout(RESOLVE_IP_TIMEOUT, upnp_ip(client)).await {
            Ok(Ok(ip)) => {
                tracing::info!(%ip, "Resolved external ip addr using ipfy service");
                return Ok(ip);
            }
            Ok(Err(e)) => {
                tracing::warn!("Upnp external ip action errored: {e}")
            }
            Err(_) => {
                tracing::warn!("Upnp external ip action timed out");
            }
        };
    }
    Err(anyhow::anyhow!("Failed to resolve external ip address"))
}

async fn ipfy_ip(client: reqwest::Client) -> anyhow::Result<Ipv4Addr> {
    client
        .get("https://api.ipify.org")
        .timeout(RESOLVE_IP_TIMEOUT)
        .send()
        .await?
        .text()
        .await?
        .parse()
        .context("parse ipify ip addr")
}

async fn upnp_ip(client: &ScpdClient<InternetGatewayClient>) -> anyhow::Result<Ipv4Addr> {
    let ip = client.get_external_ip_addr().await?;
    // TODO: use IpAddrV4::is_global when it becomes stable
    anyhow::ensure!(
        !ip.is_unspecified() && !ip.is_link_local() && !ip.is_multicast() && !ip.is_loopback()
    );
    Ok(ip)
}

pub async fn search_upnp_gateway() -> anyhow::Result<ScpdClient<InternetGatewayClient>> {
    let search_client = search_client::SearchClient::bind().await?;
    let service = search_client
        .search_for::<InternetGatewayClient>(search_client::SearchOptions::new())
        .await?;
    service
        .into_iter()
        .next()
        .context("find at least one internet gateway client")
}
