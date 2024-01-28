use std::net::SocketAddrV4;

use tokio::net::{TcpListener, TcpStream, UdpSocket};

pub fn verify_sha1(hash: [u8; 20], input: &[u8]) -> bool {
    use sha1::{Digest, Sha1};
    let mut hasher = <Sha1 as sha1::Digest>::new();
    hasher.update(&input);
    let result: [u8; 20] = hasher.finalize().try_into().unwrap();
    hash == result
}
