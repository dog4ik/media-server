use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::Path,
};

pub async fn clear_directory(dir: impl AsRef<Path>) -> Result<usize, io::Error> {
    use tokio::fs;
    let mut removed_files = 0;
    let mut directory = fs::read_dir(dir).await?;
    while let Ok(Some(file)) = directory.next_entry().await {
        if fs::remove_file(file.path()).await.is_ok() {
            removed_files += 1;
        } else {
            tracing::error!("Failed to remove file: {}", file.path().display());
        };
    }
    Ok(removed_files)
}

#[tracing::instrument(level = "debug")]
pub async fn local_addr() -> std::io::Result<SocketAddr> {
    use tokio::net::UdpSocket;
    const SSDP_IP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
    const SSDP_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(SSDP_IP_ADDR, 1900));
    let socket =
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).await?;
    socket.connect(SSDP_ADDR).await?;
    socket.local_addr()
}

pub fn stringify_info_hash(hash: &[u8; 20]) -> String {
    hash.iter().fold(String::with_capacity(40), |mut acc, x| {
        acc += &format!("{:02x}", x);
        acc
    })
}

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x08000000;
