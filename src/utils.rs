use crc32fast::Hasher;
use std::{
    fs::{self, File},
    io::{Read, self},
    path::{PathBuf, Path},
};

pub fn file_hash(file: &mut File) -> Result<u32, std::io::Error> {
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

    return Ok(result);
}

pub fn walk_recursive<F>(
    folder: &PathBuf,
    filter_fn: Option<F>,
) -> Result<Vec<PathBuf>, std::io::Error>
where
    F: Fn(&PathBuf) -> bool + std::marker::Copy,
{
    let mut local_paths = Vec::new();
    let dir = fs::read_dir(&folder)?;
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
            local_paths.append(walk_recursive(&path.to_path_buf(), filter_fn)?.as_mut());
        }
    }
    return Ok(local_paths);
}

pub async fn clear_directory(dir: impl AsRef<Path>) -> Result<usize, io::Error> {
    use tokio::fs;
    let mut removed_files = 0;
    let mut directory = fs::read_dir(dir).await?;
    while let Ok(Some(file)) = directory.next_entry().await {
        if fs::remove_file(file.path()).await.is_ok() {
            removed_files += 1;
        } else {
            tracing::error!("Failed to remove file: {:?}", file.path());
        };
    }
    Ok(removed_files)
}