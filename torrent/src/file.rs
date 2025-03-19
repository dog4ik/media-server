use std::path::Path;

use reqwest::Url;

use crate::protocol::Info;

#[derive(Debug)]
pub struct TorrentFile {
    pub info: Info,
    /// The URL of the tracker.
    pub announce: String,
    pub encoding: Option<String>,
    /// List of trackers
    pub announce_list: Option<Vec<Vec<String>>>,
    pub creation_date: Option<u64>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
}

impl bendy::decoding::FromBencode for TorrentFile {
    fn decode_bencode_object(
        object: bendy::decoding::Object,
    ) -> Result<Self, bendy::decoding::Error> {
        use bendy::decoding::Error;
        use bendy::decoding::ResultExt;

        let mut announce = None;
        let mut announce_list = None;
        let mut encoding = None;
        let mut comment = None;
        let mut creation_date = None;
        let mut created_by = None;
        // let mut http_seeds = None;
        let mut info = None;

        let mut dict_dec = object.try_into_dictionary()?;
        while let Some((tag, value)) = dict_dec.next_pair()? {
            match tag {
                b"announce" => {
                    announce = String::decode_bencode_object(value)
                        .context("announce")
                        .map(Some)?;
                }
                b"announce-list" => {
                    announce_list = Vec::decode_bencode_object(value)
                        .context("announce-list")
                        .map(Some)?;
                }
                b"comment" => {
                    comment = String::decode_bencode_object(value)
                        .context("comment")
                        .map(Some)?;
                }
                b"creation date" => {
                    creation_date = u64::decode_bencode_object(value)
                        .context("creation_date")
                        .map(Some)?;
                }
                b"created by" => {
                    created_by = String::decode_bencode_object(value)
                        .context("created_by")
                        .map(Some)?;
                }
                b"encoding" => {
                    encoding = String::decode_bencode_object(value)
                        .context("encoding")
                        .map(Some)?;
                }
                b"info" => {
                    info = Info::decode_bencode_object(value)
                        .context("info")
                        .map(Some)?;
                }
                _ => {
                    tracing::warn!(
                        "Unexpected field in .torrent file: {}",
                        String::from_utf8_lossy(tag)
                    );
                }
            }
        }

        let announce = announce.ok_or_else(|| Error::missing_field("announce"))?;
        let info = info.ok_or_else(|| Error::missing_field("info"))?;

        Ok(Self {
            announce,
            announce_list,
            info,
            encoding,
            comment,
            creation_date,
            created_by,
        })
    }
}

impl TorrentFile {
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        bendy::decoding::FromBencode::from_bencode(bytes.as_ref())
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        use std::fs;
        let bytes = fs::read(path)?;
        let torrent = Self::from_bytes(bytes)?;
        Ok(torrent)
    }

    /// Get all trackers contained in file
    pub fn all_trackers(&self) -> Vec<Url> {
        let mut trackers =
            Vec::with_capacity(1 + self.announce_list.as_ref().map_or(0, |l| l.len()));
        if let Ok(url) = Url::parse(&self.announce) {
            trackers.push(url);
        } else {
            tracing::error!(
                self.announce,
                "failed to parce announce url in .torrent file"
            );
        }
        if let Some(list) = &self.announce_list {
            trackers.extend(list.iter().flatten().filter_map(|url| Url::parse(url).ok()));
        };
        trackers
    }
}

#[cfg(test)]
mod tests {

    use crate::file::TorrentFile;

    #[test]
    fn parse_torrent_file() {
        let torrent_file = TorrentFile::from_bytes(include_bytes!("../sample.torrent")).unwrap();
        assert_eq!(
            torrent_file.announce,
            "http://bittorrent-test-tracker.codecrafters.io/announce"
        );
        assert_eq!(torrent_file.info.name, "sample.txt");
        assert_eq!(torrent_file.created_by.unwrap(), "mktorrent 1.1");
        assert_eq!(torrent_file.info.total_size(), 92063);
        assert_eq!(
            torrent_file.info.hex_hash(),
            "d69f91e6b2ae4c542468d1073a71d4ea13879a7f"
        );
    }
}
