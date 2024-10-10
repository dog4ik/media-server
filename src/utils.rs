use std::{
    fs::{self, File},
    io::{self, Read},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::{Path, PathBuf},
};

pub fn file_hash(file: &mut File) -> Result<u32, std::io::Error> {
    use crc32fast::Hasher;
    let mut hasher = Hasher::new();
    let mut buffer = [0; 4096];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    let result = hasher.finalize();

    Ok(result)
}

pub fn walk_recursive<F>(
    folder: impl AsRef<Path>,
    filter_fn: Option<F>,
) -> Result<Vec<PathBuf>, std::io::Error>
where
    F: Fn(&PathBuf) -> bool + std::marker::Copy,
{
    let mut local_paths = Vec::new();
    let dir = fs::read_dir(folder)?;
    for file in dir {
        let path = file?.path();
        if path.is_file() {
            if let Some(filter_fn) = filter_fn {
                if filter_fn(&path) {
                    local_paths.push(path);
                }
            } else {
                local_paths.push(path);
            }
        } else if path.is_dir() {
            local_paths.append(walk_recursive(&path, filter_fn)?.as_mut());
        }
    }
    Ok(local_paths)
}

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

pub fn tokenize_filename(file_name: &str) -> Vec<String> {
    let is_spaced = file_name.contains(' ');
    match is_spaced {
        true => file_name.split(' '),
        false => file_name.split('.'),
    }
    .map(|e| e.trim().to_lowercase())
    .filter(|t| t != "-")
    .collect()
}

pub fn calculate_sha256<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S> + Copy,
    S: AsRef<[u8]>,
{
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    for arg in args {
        hasher.update(arg);
    }

    format!("{:x}", hasher.finalize())
}

pub async fn local_addr() -> std::io::Result<SocketAddr> {
    use tokio::net::UdpSocket;
    const SSDP_IP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
    const SSDP_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(SSDP_IP_ADDR, 1900));
    let socket =
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).await?;
    socket.connect(SSDP_ADDR).await?;
    socket.local_addr()
}
