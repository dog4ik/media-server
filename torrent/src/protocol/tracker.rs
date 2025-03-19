use std::{
    io::{Cursor, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};

use anyhow::Context;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Deserialize, Serialize, Debug, Copy, Clone)]
#[serde(rename_all = "lowercase")]
pub enum TrackerEvent {
    Started,
    Completed,
    Stopped,
    Empty,
}

impl TrackerEvent {
    pub fn as_bytes(&self) -> [u8; 4] {
        match self {
            TrackerEvent::Started => 2_u32,
            TrackerEvent::Completed => 1_u32,
            TrackerEvent::Stopped => 3_u32,
            TrackerEvent::Empty => 0_u32,
        }
        .to_be_bytes()
    }
}
#[derive(Debug, Clone)]
pub enum UdpTrackerRequestType {
    Connect,
    Announce {
        ///  A connection ID can be used for multiple requests.
        ///  A client can use a connection ID until one minute after it has received it.
        ///  Trackers should accept the connection ID until two minutes after it has been send.
        connection_id: u64,
        info_hash: [u8; 20],
        /// A string of length 20 which this downloader uses as its id.
        /// Each downloader generates its own id at random at the start of a new download.
        /// This value will also almost certainly have to be escaped.
        peer_id: [u8; 20],
        /// The total amount downloaded so far, encoded in base ten ascii.
        downloaded: u64,
        /// The number of bytes this peer still has to download, encoded in base ten ascii.
        /// Note that this can't be computed from downloaded and the file length since it might be a resume,
        /// and there's a chance that some of the downloaded data failed an integrity check and had to be re-downloaded.
        left: u64,
        /// The total amount uploaded so far, encoded in base ten ascii.
        uploaded: u64,
        /// This is an optional key which maps to started, completed, or stopped (or empty, which is the same as not being present).
        /// If not present, this is one of the announcements done at regular intervals.
        /// An announcement using started is sent when a download first begins,
        /// and one using completed is sent when the download is complete.
        /// No completed is sent if the file was complete when started.
        /// Downloaders send an announcement using stopped when they cease downloading.
        event: TrackerEvent,
        /// An optional parameter giving the IP (or dns name) which this peer is at.
        /// Generally used for the origin if it's on the same machine as the tracker.
        ip: u32,
        /// Clients that resolve hostnames to v4 and v6 and then announce to both should use
        /// the same key for both so that trackers that care about accurate statistics-keeping
        /// can match the two announces.
        key: u32,
        /// The number of peers we would like to have in announce response.
        /// Value of -1 tells tracker to decide how many peers to return.
        num_want: i32,
        /// The port number this peer is listening on.
        /// Common behavior is for a downloader to try to listen on port 6881 and if that port is taken try 6882,
        /// then 6883, etc. and give up after 6889.
        port: u16,
    },
    #[allow(unused)]
    Scrape {
        connection_id: u64,
        info_hashes: Vec<[u8; 20]>,
    },
}

#[derive(Debug)]
pub struct UdpTrackerRequest {
    pub transaction_id: u32,
    pub request_type: UdpTrackerRequestType,
    pub tracker_addr: SocketAddr,
    pub response: oneshot::Sender<UdpTrackerMessage>,
}

impl UdpTrackerRequest {
    pub fn new(
        request_type: UdpTrackerRequestType,
        tracker_addr: SocketAddr,
        response: oneshot::Sender<UdpTrackerMessage>,
    ) -> Self {
        let transaction_id = rand::random();
        Self {
            transaction_id,
            request_type,
            tracker_addr,
            response,
        }
    }

    pub fn as_bytes(&self) -> Bytes {
        match &self.request_type {
            UdpTrackerRequestType::Connect => {
                let protocol: u64 = 0x41727101980;
                let mut buffer = Cursor::new([0_u8; 16]);
                buffer.write_all(&protocol.to_be_bytes()).unwrap();
                buffer.write_all(&0_u32.to_be_bytes()).unwrap();
                buffer
                    .write_all(&self.transaction_id.to_be_bytes())
                    .unwrap();
                Bytes::copy_from_slice(&buffer.into_inner())
            }
            UdpTrackerRequestType::Announce {
                connection_id,
                info_hash,
                peer_id,
                downloaded,
                left,
                uploaded,
                event,
                ip,
                key,
                num_want,
                port,
            } => {
                let mut writer = Cursor::new([0_u8; 98]);
                writer.write_all(&connection_id.to_be_bytes()).unwrap();
                writer.write_all(&1_u32.to_be_bytes()).unwrap();
                writer
                    .write_all(&self.transaction_id.to_be_bytes())
                    .unwrap();
                writer.write_all(info_hash).unwrap();
                writer.write_all(peer_id).unwrap();
                writer.write_all(&downloaded.to_be_bytes()).unwrap();
                writer.write_all(&left.to_be_bytes()).unwrap();
                writer.write_all(&uploaded.to_be_bytes()).unwrap();
                writer.write_all(&event.as_bytes()).unwrap();
                writer.write_all(&ip.to_be_bytes()).unwrap();
                writer.write_all(&key.to_be_bytes()).unwrap();
                writer.write_all(&num_want.to_be_bytes()).unwrap();
                writer.write_all(&port.to_be_bytes()).unwrap();
                Bytes::copy_from_slice(&writer.into_inner())
            }
            UdpTrackerRequestType::Scrape {
                connection_id,
                info_hashes,
            } => {
                let bytes = BytesMut::new();
                let mut writer = bytes.writer();
                writer.write_all(&connection_id.to_be_bytes()).unwrap();
                writer.write_all(&2_u32.to_be_bytes()).unwrap();
                writer
                    .write_all(&self.transaction_id.to_be_bytes())
                    .unwrap();
                for info_hash in info_hashes {
                    writer.write_all(info_hash).unwrap();
                }
                writer.into_inner().into()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct UdpTrackerMessage {
    pub transaction_id: u32,
    pub message_type: UdpTrackerMessageType,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct UdpScrapeUnit {
    pub seeders: u32,
    pub completed: u32,
    pub leechers: u32,
}

#[derive(Debug, Clone)]
pub enum UdpTrackerMessageType {
    Connect {
        connection_id: u64,
    },
    Announce {
        interval: u32,
        leechers: u32,
        seeders: u32,
        peers: Vec<SocketAddr>,
    },
    #[allow(unused)]
    Scrape {
        units: Vec<UdpScrapeUnit>,
    },
    Error {
        message: String,
    },
}

fn read_u32(reader: &mut impl Read) -> Option<u32> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf).ok()?;
    Some(u32::from_be_bytes(buf))
}

fn read_u64(reader: &mut impl Read) -> Option<u64> {
    let mut buf = [0; 8];
    reader.read_exact(&mut buf).ok()?;
    Some(u64::from_be_bytes(buf))
}

impl UdpTrackerMessage {
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut cursor = Cursor::new(bytes);

        let action = read_u32(&mut cursor).context("message doesn't contain action byte")?;
        let transaction_id = read_u32(&mut cursor).context("read transaction id")?;

        let message_type = match action {
            0 => {
                let connection_id = read_u64(&mut cursor).context("read connection id")?;
                UdpTrackerMessageType::Connect { connection_id }
            }
            1 => {
                let interval = read_u32(&mut cursor).context("read interval")?;
                let leechers = read_u32(&mut cursor).context("read leechers")?;
                let seeders = read_u32(&mut cursor).context("read seeders")?;
                let mut remaining = vec![0; cursor.remaining()];
                cursor.read_exact(&mut remaining).unwrap();
                let ips_len = remaining.len().checked_div(6).context("ip's dont add up")?;
                let mut ips = Vec::with_capacity(ips_len);
                for slice in remaining.array_chunks::<6>() {
                    let ip = u32::from_be_bytes(slice[0..4].try_into().unwrap());
                    let port = u16::from_be_bytes(slice[4..6].try_into().unwrap());
                    let ip = Ipv4Addr::from_bits(ip);
                    ips.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
                }
                UdpTrackerMessageType::Announce {
                    interval,
                    leechers,
                    seeders,
                    peers: ips,
                }
            }
            2 => {
                let mut remaining = vec![0; cursor.remaining()];
                cursor.read_exact(&mut remaining).unwrap();
                let res_len = remaining.len().checked_div(12).context("incomplete data")?;
                let mut units = Vec::with_capacity(res_len);
                for slice in remaining.array_chunks::<12>() {
                    let mut reader = Cursor::new(slice);
                    let seeders = read_u32(&mut reader).unwrap();
                    let completed = read_u32(&mut reader).unwrap();
                    let leechers = read_u32(&mut reader).unwrap();
                    units.push(UdpScrapeUnit {
                        seeders,
                        completed,
                        leechers,
                    });
                }
                UdpTrackerMessageType::Scrape { units }
            }
            3 => {
                let mut remaining = vec![0; cursor.remaining()];
                cursor.read_exact(&mut remaining).unwrap();
                let message = String::from_utf8(remaining)?;
                UdpTrackerMessageType::Error { message }
            }
            rest => return Err(anyhow::anyhow!("Action {} is not recognized", rest)),
        };
        Ok(UdpTrackerMessage {
            transaction_id,
            message_type,
        })
    }
}
