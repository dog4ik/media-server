use std::net::SocketAddrV4;

use tokio::net::{TcpListener, UdpSocket};

pub fn verify_sha1(hash: [u8; 20], input: &[u8]) -> bool {
    use sha1::{Digest, Sha1};
    let mut hasher = <Sha1 as sha1::Digest>::new();
    hasher.update(input);
    let result: [u8; 20] = hasher.finalize().into();
    hash == result
}

pub fn piece_size(piece_i: usize, piece_length: usize, total_length: usize) -> usize {
    let total_pieces = (total_length + piece_length - 1) / piece_length;

    if piece_i == total_pieces - 1 {
        let md = total_length % piece_length;
        if md == 0 {
            piece_length
        } else {
            md
        }
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
