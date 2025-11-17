use std::fmt::Display;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StorageError {
    Fs(std::io::ErrorKind),
    Hash,
    Bounds,
    MissingPiece,
}

impl std::error::Error for StorageError {}

impl Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Fs(e) => {
                write!(f, "fs error ({e})")
            }
            StorageError::Hash => {
                write!(f, "hash validation error")
            }
            StorageError::Bounds => {
                write!(f, "piece bounds error")
            }
            StorageError::MissingPiece => {
                write!(f, "missing pieces error")
            }
        }
    }
}

impl From<std::io::Error> for StorageError {
    fn from(value: std::io::Error) -> Self {
        Self::Fs(value.kind())
    }
}
