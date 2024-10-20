use std::net::SocketAddrV4;

use tokio::net::{TcpListener, UdpSocket};

pub fn verify_sha1(hash: [u8; 20], input: &[u8]) -> bool {
    use sha1::{Digest, Sha1};
    let mut hasher = <Sha1 as sha1::Digest>::new();
    hasher.update(input);
    let result: [u8; 20] = hasher.finalize().into();
    hash == result
}

pub fn verify_iter_sha1(hash: [u8; 20], input: impl Iterator<Item = impl AsRef<[u8]>>) -> bool {
    use sha1::{Digest, Sha1};
    let mut hasher = <Sha1 as sha1::Digest>::new();
    for el in input {
        hasher.update(el);
    }
    let result: [u8; 20] = hasher.finalize().into();
    hash == result
}

pub fn piece_size(piece_i: usize, piece_length: u32, total_length: u64) -> u64 {
    let piece_length = piece_length as u64;
    let total_pieces = total_length.div_ceil(piece_length);

    if piece_i == total_pieces as usize - 1 {
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
