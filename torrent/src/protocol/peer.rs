use std::{
    collections::HashMap,
    fmt::Display,
    hash::Hasher,
    io::{Read, Write},
    net::SocketAddr,
};

use anyhow::{Context, anyhow, ensure};
use bytes::{Buf, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::codec::Decoder;

use crate::{
    CLIENT_NAME,
    bitfield::BitField,
    download::{Block, PEER_IN_CHANNEL_CAPACITY},
};

use super::{extension::Extension, pex, ut_metadata};

#[derive(Debug, Clone)]
pub struct PeerId(pub [u8; 20]);

#[derive(Debug, Clone, Default)]
pub struct PeerFP {
    name: Box<[u8]>,
    major: u32,
    minor: u32,
    revision: u32,
    tag: u32,
}

impl std::fmt::Display for PeerFP {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{name} {major}.{minor}.{revision}.{tag}",
            name = self.client_name(),
            major = self.major,
            minor = self.minor,
            revision = self.revision,
            tag = self.tag,
        )
    }
}

impl PeerFP {
    fn parse_azure_style(id: &[u8; 20]) -> anyhow::Result<Self> {
        // These macros early return if condition is not satisfied
        let dash = b"-"[0];
        anyhow::ensure!(id[0] == dash, "first byte must be dash");
        anyhow::ensure!(id[7] == dash, "8th byte must be dash");

        anyhow::ensure!(id[1].is_ascii());
        anyhow::ensure!(id[2].is_ascii());

        let name: [u8; 2] = [id[1], id[2]];
        let major = char::from(id[3]).to_digit(10).context("parse major")?;
        let minor = char::from(id[4]).to_digit(10).context("parse minor")?;
        let revision = char::from(id[5]).to_digit(10).context("parse revision")?;
        let tag = char::from(id[6]).to_digit(10).context("parse tag")?;

        Ok(Self {
            name: Box::new(name),
            major,
            minor,
            revision,
            tag,
        })
    }

    fn parse_shadow_style(id: &[u8; 20]) -> anyhow::Result<Self> {
        let first = char::from(id[0]);
        anyhow::ensure!(first.is_alphanumeric());
        let major;
        let minor;
        let revision;
        if &id[4..6] == b"--" {
            major = char::from(id[1]).to_digit(10).context("major version")?;
            minor = char::from(id[2]).to_digit(10).context("minor version")?;
            revision = char::from(id[3]).to_digit(10).context("revision version")?;
        } else {
            anyhow::ensure!(id[8] == 0);
            anyhow::ensure!(id[1] <= 127);
            anyhow::ensure!(id[2] <= 127);
            anyhow::ensure!(id[3] <= 127);
            major = id[1] as u32;
            minor = id[2] as u32;
            revision = id[3] as u32;
        }

        let tag = 0;
        Ok(Self {
            name: Box::new([id[0]]),
            major,
            minor,
            revision,
            tag,
        })
    }

    fn parse_mainline_style(id: &[u8; 20]) -> anyhow::Result<Self> {
        let str = String::from_utf8(id.to_vec())?;
        let (first_char, rest) = str
            .chars()
            .next()
            .zip(str.get(1..))
            .context("split off first char")?;
        anyhow::ensure!(first_char.is_ascii_graphic());
        let parts: Vec<_> = rest.split('-').collect();
        anyhow::ensure!(parts.len() == 4);
        anyhow::ensure!(parts[3] == "--");
        anyhow::ensure!(parts[0].len() == 3);
        anyhow::ensure!(parts[1].len() == 3);
        anyhow::ensure!(parts[2].len() == 3);

        let major = parts[0].parse().context("parse major")?;
        let minor = parts[1].parse().context("parse minor")?;
        let revision = parts[2].parse().context("parse revision")?;
        Ok(Self {
            name: Box::new([first_char as u8]),
            major,
            minor,
            revision,
            tag: 0,
        })
    }

    pub fn client_name(&self) -> &'static str {
        match &self.name[..] {
            b"7T" => "aTorrent for android",
            b"A" => "ABC",
            b"AB" => "AnyEvent BitTorrent",
            b"AG" => "Ares",
            b"AR" => "Arctic Torrent",
            b"AT" => "Artemis",
            b"AV" => "Avicora",
            b"AX" => "BitPump",
            b"AZ" => "Azureus",
            b"A~" => "Ares",
            b"BB" => "BitBuddy",
            b"BC" => "BitComet",
            b"BE" => "baretorrent",
            b"BF" => "Bitflu",
            b"BG" => "BTG",
            b"BI" => "BiglyBT",
            b"BL" => "BitBlinder",
            b"BP" => "BitTorrent Pro",
            b"BR" => "BitRocket",
            b"BS" => "BTSlave",
            b"BT" => "BitTorrent",
            b"BU" => "BigUp",
            b"BW" => "BitWombat",
            b"BX" => "BittorrentX",
            b"CD" => "Enhanced CTorrent",
            b"CT" => "CTorrent",
            b"DE" => "Deluge",
            b"DP" => "Propagate Data Client",
            b"EB" => "EBit",
            b"ES" => "electric sheep",
            b"FC" => "FileCroc",
            b"FT" => "FoxTorrent",
            b"FW" => "FrostWire",
            b"FX" => "Freebox BitTorrent",
            b"GS" => "GSTorrent",
            b"HK" => "Hekate",
            b"HL" => "Halite",
            b"HN" => "Hydranode",
            b"IL" => "iLivid",
            b"KC" => "Koinonein",
            b"KG" => "KGet",
            b"KT" => "KTorrent",
            b"LC" => "LeechCraft",
            b"LH" => "LH-ABC",
            b"LK" => "Linkage",
            b"LP" => "lphant",
            b"LR" => "LibreTorrent",
            b"LT" => "libtorrent",
            b"LW" => "Limewire",
            b"M" => "Mainline",
            b"ML" => "MLDonkey",
            b"MO" => "Mono Torrent",
            b"MP" => "MooPolice",
            b"MR" => "Miro",
            b"MT" => "Moonlight Torrent",
            b"NX" => "Net Transport",
            b"O" => "Osprey Permaseed",
            b"OS" => "OneSwarm",
            b"OT" => "OmegaTorrent",
            b"PD" => "Pando",
            b"Q" => "BTQueue",
            b"QD" => "QQDownload",
            b"QT" => "Qt 4",
            b"R" => "Tribler",
            b"RT" => "Retriever",
            b"RZ" => "RezTorrent",
            b"S" => "Shadow",
            b"SB" => "Swiftbit",
            b"SD" => "Xunlei",
            b"SK" => "spark",
            b"SN" => "ShareNet",
            b"SS" => "SwarmScope",
            b"ST" => "SymTorrent",
            b"SZ" => "Shareaza",
            b"S~" => "Shareaza (beta)",
            b"T" => "BitTornado",
            b"TB" => "Torch",
            b"TL" => "Tribler",
            b"TN" => "Torrent.NET",
            b"TR" => "Transmission",
            b"TS" => "TorrentStorm",
            b"TT" => "TuoTu",
            b"U" => "UPnP",
            b"UL" => "uLeecher",
            b"UM" => "uTorrent Mac",
            b"UT" => "uTorrent",
            b"VG" => "Vagaa",
            b"WT" => "BitLet",
            b"WY" => "FireTorrent",
            b"XF" => "Xfplay",
            b"XL" => "Xunlei",
            b"XS" => "XSwifter",
            b"XT" => "XanTorrent",
            b"XX" => "Xtorrent",
            b"ZO" => "Zona",
            b"ZT" => "ZipTorrent",
            b"lt" => "rTorrent",
            b"pX" => "pHoeniX",
            b"qB" => "qBittorrent",
            b"st" => "SharkTorrent",
            _ => "Unknown",
        }
    }
}

impl TryFrom<&[u8; 20]> for PeerFP {
    type Error = anyhow::Error;

    fn try_from(value: &[u8; 20]) -> Result<Self, Self::Error> {
        Self::parse_azure_style(value)
            .or_else(|_| Self::parse_shadow_style(value))
            .or_else(|_| Self::parse_mainline_style(value))
    }
}

impl PeerId {
    pub fn my_id() -> Self {
        let mut id: [u8; 20] = rand::random();
        id[0] = b"-"[0];
        (id[1], id[2]) = (b"M"[0], b"S"[0]);
        id[3] = b"1"[0];
        id[4] = b"0"[0];
        id[5] = b"0"[0];
        id[6] = b"0"[0];
        Self(id)
    }

    pub fn client_name(&self) -> &'static str {
        PeerFP::try_from(&self.0)
            .map(|i| i.client_name())
            .unwrap_or("unknown")
    }
}

#[derive(Debug, Clone)]
pub struct HandShake {
    pub reserved: [u8; 8],
    pub info_hash: [u8; 20],
    pub peer_id: PeerId,
}

impl HandShake {
    pub const SIZE: usize = 68;

    pub fn new(info_hash: [u8; 20]) -> Self {
        let mut reserved = [0_u8; 8];
        // support extensions
        reserved[5] = 0x10;

        Self {
            info_hash,
            reserved,
            peer_id: PeerId::my_id(),
        }
    }

    pub fn supports_extensions(&self) -> bool {
        self.reserved[5] & 0x10 != 0
    }

    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let length = bytes.first().context("length byte is not set")?;
        ensure!(*length == 19);

        ensure!(bytes.len() > 20);
        ensure!(&bytes[1..20] == b"BitTorrent protocol");
        let mut bytes = &bytes[20..];

        let mut reserved = [0; 8];
        let mut info_hash = [0; 20];
        let mut peer_id = [0; 20];
        bytes.read_exact(&mut reserved).context("reserved bytes")?;
        bytes.read_exact(&mut info_hash).context("hash bytes")?;
        bytes.read_exact(&mut peer_id).context("peer_id bytes")?;
        debug_assert!(bytes.is_empty());

        Ok(Self {
            reserved,
            peer_id: PeerId(peer_id),
            info_hash,
        })
    }

    pub fn as_bytes(&self) -> [u8; 68] {
        let mut out = [0_u8; 68];
        let mut slice = &mut out[..];

        slice.write_all(&[19]).unwrap();
        slice.write_all(b"BitTorrent protocol").unwrap();
        slice.write_all(&self.reserved).unwrap();
        slice.write_all(&self.info_hash).unwrap();
        slice.write_all(&self.peer_id.0).unwrap();
        debug_assert!(slice.is_empty());
        out
    }

    pub fn info_hash(&self) -> [u8; 20] {
        self.info_hash
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionHandshake {
    #[serde(rename = "m")]
    pub dict: HashMap<String, u8>,
    #[serde(flatten)]
    pub fields: HashMap<String, serde_bencode::value::Value>,
}

pub const CLIENT_EXTENSIONS: [(&str, u8); 2] = [
    (
        ut_metadata::UtMessage::NAME,
        ut_metadata::UtMessage::CLIENT_ID,
    ),
    (pex::PexMessage::NAME, pex::PexMessage::CLIENT_ID),
];

impl ExtensionHandshake {
    pub fn from_bytes(bytes: &[u8]) -> serde_bencode::Result<Self> {
        serde_bencode::from_bytes(bytes)
    }

    pub fn as_bytes(&self) -> Bytes {
        serde_bencode::to_bytes(self).unwrap().into()
    }

    pub fn my_handshake() -> Self {
        let mut dict = HashMap::with_capacity(CLIENT_EXTENSIONS.len());
        let mut fields = HashMap::new();
        for (name, id) in CLIENT_EXTENSIONS {
            dict.insert(name.into(), id);
        }

        fields.insert(
            "reqq".to_string(),
            serde_bencode::value::Value::Int(PEER_IN_CHANNEL_CAPACITY as i64),
        );

        //fields.insert(
        //    "p".to_string(),
        //    serde_bencode::value::Value::Int(tcp_listener_port as i64),
        //);

        fields.insert(
            "v".to_string(),
            serde_bencode::value::Value::Bytes(CLIENT_NAME.as_bytes().to_vec()),
        );

        // TODO: add client's `v` (client name), and `p` (tcp listener port) fields

        Self { dict, fields }
    }

    /// Metadata size if it supports ut_metadata
    pub fn ut_metadata_size(&self) -> Option<usize> {
        self.fields
            .get("metadata_size")
            .and_then(|size| match size {
                serde_bencode::value::Value::Int(size) => Some(usize::try_from(*size).ok()?),
                _ => None,
            })
    }

    /// Pex's extenison id if handshake supports it
    pub fn pex_id(&self) -> Option<u8> {
        self.dict.get("ut_pex").copied()
    }

    /// Ut_metadata's extenison id if handshake supports it
    pub fn ut_metadata_id(&self) -> Option<u8> {
        self.dict.get("ut_metadata").copied()
    }

    /// A string containing the compact representation of the ip address this peer sees you as.
    /// i.e. this is the receiver's external ip address (no port is included).
    /// This may be either an IPv4 (4 bytes) or an IPv6 (16 bytes) address.
    pub fn your_ip(&self) -> Option<std::net::IpAddr> {
        let serde_bencode::value::Value::Bytes(bytes) = self.fields.get("yourip")? else {
            return None;
        };
        bytes
            .get(..4)
            .map(|v| TryInto::<[u8; 4]>::try_into(v).unwrap())
            .map(std::net::IpAddr::from)
            .or_else(|| {
                bytes
                    .get(..16)
                    .map(|v| TryInto::<[u8; 16]>::try_into(v).unwrap())
                    .map(std::net::IpAddr::from)
            })
    }

    pub fn set_your_ip(&mut self, peer_ip: std::net::IpAddr) {
        let ip_bytes = match peer_ip {
            std::net::IpAddr::V4(ipv4_addr) => &ipv4_addr.octets()[..],
            std::net::IpAddr::V6(ipv6_addr) => &ipv6_addr.octets()[..],
        };
        self.fields.insert(
            "yourip".to_owned(),
            serde_bencode::value::Value::Bytes(ip_bytes.to_vec()),
        );
    }

    /// The maximum number of outstanding request messages this client supports without dropping any.  
    /// The default in libtorrent was 250.  
    /// However, as of 2025, 250 is outdated, and the new standard is 2000.
    /// TODO: verify the above
    pub fn request_queue_size(&self) -> Option<i64> {
        let serde_bencode::value::Value::Int(size) = self.fields.get("reqq")? else {
            return None;
        };
        Some(*size)
    }

    /// Client name and version (as a utf-8 string).
    /// This is a much more reliable way of identifying the client than relying on the peer id encoding.
    pub fn client_name(&self) -> Option<&str> {
        let serde_bencode::value::Value::Bytes(bytes) = self.fields.get("v")? else {
            return None;
        };
        std::str::from_utf8(bytes).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerMessage {
    HeartBeat,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        index: u32,
    },
    Bitfield {
        payload: BitField,
    },
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Bytes,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    ExtensionHandshake {
        payload: ExtensionHandshake,
    },
    Extension {
        extension_id: u8,
        payload: Bytes,
    },
}

impl Display for PeerMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerMessage::HeartBeat => write!(f, "HeatBeat"),
            PeerMessage::Choke => write!(f, "Choke"),
            PeerMessage::Unchoke => write!(f, "Unchoke"),
            PeerMessage::Interested => write!(f, "Interested"),
            PeerMessage::NotInterested => write!(f, "NotInterested"),
            PeerMessage::Have { index } => write!(f, "Have {}", index),
            PeerMessage::Bitfield { payload } => {
                write!(f, "Bitfield with length {}", payload.0.len())
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => write!(
                f,
                "Request for piece {index} with offset {begin} and length {length}"
            ),
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => write!(
                f,
                "Block for piece {index} with offset {begin} and length {}",
                block.len()
            ),
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => write!(
                f,
                "Cancel for piece {index} with offset {begin} and length {length}",
            ),
            PeerMessage::ExtensionHandshake { .. } => {
                write!(f, "Extension handshake")
            }
            PeerMessage::Extension { extension_id, .. } => {
                let name = CLIENT_EXTENSIONS
                    .iter()
                    .find(|(_, id)| id == extension_id)
                    .map(|(name, _)| *name)
                    .unwrap_or("unknown");
                write!(f, "{name} extension with id {extension_id}")
            }
        }
    }
}

impl PeerMessage {
    pub fn from_frame(frame: Bytes) -> anyhow::Result<Self> {
        if frame.is_empty() {
            return Ok(Self::HeartBeat);
        }
        let request_payload = |mut b: &[u8]| -> anyhow::Result<_> {
            let mut index_buffer = [0; 4];
            let mut begin_buffer = [0; 4];
            let mut length_buffer = [0; 4];
            b.read_exact(&mut index_buffer).context("index buffer")?;
            b.read_exact(&mut begin_buffer).context("begin buffer")?;
            b.read_exact(&mut length_buffer).context("length buffer")?;
            Ok((
                u32::from_be_bytes(index_buffer),
                u32::from_be_bytes(begin_buffer),
                u32::from_be_bytes(length_buffer),
            ))
        };
        let tag = frame[0];
        let payload = &frame[1..];
        match tag {
            0 => Ok(PeerMessage::Choke),
            1 => Ok(PeerMessage::Unchoke),
            2 => Ok(PeerMessage::Interested),
            3 => Ok(PeerMessage::NotInterested),
            4 => {
                let index_buffer = payload[0..4].try_into()?;

                Ok(PeerMessage::Have {
                    index: u32::from_be_bytes(index_buffer),
                })
            }
            5 => {
                let payload = BitField::new(payload);
                Ok(PeerMessage::Bitfield { payload })
            }
            6 => {
                let (index, begin, length) = request_payload(payload)?;
                Ok(PeerMessage::Request {
                    index,
                    length,
                    begin,
                })
            }
            7 => {
                let index_buffer: [u8; 4] = payload[0..4].try_into()?;
                let begin_buffer: [u8; 4] = payload[4..8].try_into()?;
                let index = u32::from_be_bytes(index_buffer);
                let begin = u32::from_be_bytes(begin_buffer);
                let block = frame.slice(9..);
                Ok(PeerMessage::Piece {
                    index,
                    begin,
                    block,
                })
            }
            8 => {
                let (index, begin, length) = request_payload(payload)?;
                Ok(PeerMessage::Cancel {
                    index,
                    length,
                    begin,
                })
            }
            20 => {
                let extension_id = payload[0];
                if extension_id == 0 {
                    Ok(PeerMessage::ExtensionHandshake {
                        payload: ExtensionHandshake::from_bytes(payload[1..].as_ref())?,
                    })
                } else {
                    let payload = frame.slice(2..);
                    Ok(PeerMessage::Extension {
                        extension_id,
                        payload,
                    })
                }
            }
            t => Err(anyhow!("unsupported tag: {}", t)),
        }
    }

    pub async fn write_to<T: AsyncWrite + Unpin>(&self, mut reader: T) -> std::io::Result<()> {
        async fn write_len(mut reader: impl AsyncWrite + Unpin, len: u32) -> std::io::Result<()> {
            reader.write_u32(len).await
        }
        match self {
            PeerMessage::HeartBeat => write_len(&mut reader, 0).await,
            PeerMessage::Choke => {
                write_len(&mut reader, 1).await?;
                reader.write_u8(0).await
            }
            PeerMessage::Unchoke => {
                write_len(&mut reader, 1).await?;
                reader.write_u8(1).await
            }
            PeerMessage::Interested => {
                write_len(&mut reader, 1).await?;
                reader.write_u8(2).await
            }
            PeerMessage::NotInterested => {
                write_len(&mut reader, 1).await?;
                reader.write_u8(3).await
            }
            PeerMessage::Have { index } => {
                write_len(&mut reader, 1 + 4).await?;
                reader.write_u8(4).await?;
                reader.write_u32(*index).await
            }
            PeerMessage::Bitfield { payload } => {
                write_len(&mut reader, 1 + payload.0.len() as u32).await?;
                reader.write_u8(5).await?;
                reader.write_all(&payload.0).await
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                write_len(&mut reader, 1 + 4 + 4 + 4).await?;
                reader.write_u8(6).await?;
                reader.write_u32(*index).await?;
                reader.write_u32(*begin).await?;
                reader.write_u32(*length).await
            }
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => {
                write_len(&mut reader, 1 + 4 + 4 + block.len() as u32).await?;
                reader.write_u8(7).await?;
                reader.write_u32(*index).await?;
                reader.write_u32(*begin).await?;
                reader.write_all(block).await
            }
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => {
                write_len(&mut reader, 1 + 4 + 4 + 4).await?;
                reader.write_u8(8).await?;
                reader.write_u32(*index).await?;
                reader.write_u32(*begin).await?;
                reader.write_u32(*length).await
            }
            PeerMessage::ExtensionHandshake { payload } => {
                let payload = payload.as_bytes();
                write_len(&mut reader, 1 + 1 + payload.len() as u32).await?;
                reader.write_u8(20).await?;
                reader.write_u8(0).await?;
                reader.write_all(&payload).await
            }
            PeerMessage::Extension {
                extension_id,
                payload,
            } => {
                write_len(&mut reader, 1 + 1 + payload.len() as u32).await?;
                reader.write_u8(20).await?;
                reader.write_u8(*extension_id).await?;
                reader.write_all(payload).await
            }
        }
    }

    pub fn request(block: Block) -> Self {
        Self::Request {
            index: block.piece,
            begin: block.offset,
            length: block.length,
        }
    }
}

#[derive(Debug)]
pub struct MessageFramer;

const MAX: usize = 1 << 16;

impl Decoder for MessageFramer {
    type Item = PeerMessage;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            // Not enough data to read length marker.
            return Ok(None);
        }

        // Read length marker.
        let length = u32::from_be_bytes(src[..4].try_into().unwrap()) as usize;

        // TODO: heartbeat
        // if length == 0 {
        //     return Ok(Some(PeerMessage::HeatBeat));
        // }

        if src.len() < 5 {
            // Not enough data to read tag marker.
            return Ok(None);
        }

        // Check that the length is not too large to avoid a denial of
        // service attack where the server runs out of memory.
        if length > MAX {
            return Err(anyhow!(
                "length({}) is higher then allowed({})",
                length,
                MAX
            ));
        }

        if src.len() < 4 + length {
            // We reserve more space in the buffer. This is not strictly
            // necessary, but is a good idea performance-wise.
            src.reserve(4 + length - src.len());

            // We inform the Framed that we need more bytes to form the next
            // frame.
            return Ok(None);
        }

        let mut frame = src.split_to(4 + length);
        // skip length bytes
        frame.advance(4);
        let frame = frame.freeze();
        let message = match PeerMessage::from_frame(frame) {
            Ok(msg) => msg,
            Err(e) => return Err(anyhow!("failed to construct peer message: {}", e)),
        };

        Ok(Some(message))
    }
}

pub fn canonical_peer_priority(mut e1: SocketAddr, mut e2: SocketAddr) -> u32 {
    let mut hasher = crc32c::Crc32cHasher::new(Default::default());
    if e1.ip() == e2.ip() {
        if e1.port() > e2.port() {
            std::mem::swap(&mut e1, &mut e2);
        }
        hasher.write_u16(e1.port());
        hasher.write_u16(e2.port());
        return hasher.finish() as u32;
    }
    if let (SocketAddr::V6(mut e1), SocketAddr::V6(mut e2)) = (e1, e2) {
        if e1 > e2 {
            std::mem::swap(&mut e1, &mut e2);
        }
        let mut offset = 0xff;
        let mut b1 = e1.ip().octets();
        let mut b2 = e2.ip().octets();
        for i in 0..b1.len() {
            if offset == 0xff && b1[i] != b2[i] {
                offset = (i + 1).max(5);
            } else if i > offset {
                b1[i] &= 0x55;
                b2[i] &= 0x55;
            }
        }

        hasher.write(&b1);
        hasher.write(&b2);
        return hasher.finish() as u32;
    }

    if let (SocketAddr::V4(mut e1), SocketAddr::V4(mut e2)) = (e1, e2) {
        if e1 > e2 {
            std::mem::swap(&mut e1, &mut e2);
        }
        let mut b1 = e1.ip().octets();
        let mut b2 = e2.ip().octets();
        let mask = if b1[..2] != b2[..2] {
            [0xff, 0xff, 0x55, 0x55]
        } else if b1[..3] != b2[..3] {
            [0xff, 0xff, 0xff, 0x55]
        } else {
            [0xff, 0xff, 0xff, 0xff]
        };
        for ((b1, b2), mask) in b1.iter_mut().zip(&mut b2).zip(mask) {
            *b1 &= mask;
            *b2 &= mask;
        }

        hasher.write(&b1);
        hasher.write(&b2);
        return hasher.finish() as u32;
    }
    tracing::error!("Peer priority cannot be made of ipv4 and ipv6");
    0
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::Decoder;

    use crate::{bitfield::BitField, protocol::peer::canonical_peer_priority};

    use super::{ExtensionHandshake, MessageFramer, PeerMessage};

    #[tokio::test]
    async fn parse_peer_message() {
        async fn re_encode_message(msg: PeerMessage) {
            let mut framer = MessageFramer;
            let mut buffer = Vec::new();
            msg.write_to(&mut buffer).await.unwrap();
            let mut bytes: BytesMut = buffer.as_slice().into();
            let result = framer.decode(&mut bytes).unwrap().unwrap();
            assert_eq!(msg, result);
        }
        re_encode_message(PeerMessage::Choke).await;
        re_encode_message(PeerMessage::Unchoke).await;
        re_encode_message(PeerMessage::Interested).await;
        re_encode_message(PeerMessage::NotInterested).await;
        re_encode_message(PeerMessage::Have { index: 123 }).await;
        re_encode_message(PeerMessage::Bitfield {
            payload: BitField::empty(300),
        })
        .await;
        re_encode_message(PeerMessage::Request {
            index: 22,
            begin: 100,
            length: 200,
        })
        .await;
        re_encode_message(PeerMessage::Piece {
            index: 22,
            begin: 100,
            block: Bytes::from_static(&[23, 222, 32]),
        })
        .await;
        re_encode_message(PeerMessage::Cancel {
            index: 22,
            begin: 100,
            length: 200,
        })
        .await;
        re_encode_message(PeerMessage::ExtensionHandshake {
            payload: ExtensionHandshake::my_handshake(),
        })
        .await;
        re_encode_message(PeerMessage::Extension {
            extension_id: 1,
            payload: Bytes::from_static(&[22, 222, 32]),
        })
        .await;
    }

    #[test]
    fn peer_priority() {
        let client = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(123, 213, 32, 10)), 0);
        let peer1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(98, 76, 54, 32)), 0);
        let peer2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(123, 213, 32, 234)), 0);
        assert_eq!(canonical_peer_priority(client, peer1), 0xec2d7224);
        assert_eq!(canonical_peer_priority(client, peer2), 0x99568189);
    }
}
