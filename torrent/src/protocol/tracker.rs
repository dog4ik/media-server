use std::{
    io::{Cursor, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};

use anyhow::Context;
use bytes::{BufMut, Bytes, BytesMut};
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
        connection_id: u64,
        info_hash: [u8; 20],
        peer_id: [u8; 20],
        downloaded: u64,
        left: u64,
        uploaded: u64,
        event: TrackerEvent,
        ip: u32,
        key: u32,
        num_want: i32,
        port: u16,
    },
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

        let action = read_u32(&mut cursor).context("Message dont contain action byte")?;
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
                let remaining = cursor.remaining_slice();
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
                let remaining = cursor.remaining_slice();
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
                let message = String::from_utf8(cursor.remaining_slice().to_vec())?;
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

#[derive(Debug, Clone, Copy)]
pub struct UdpConnectRequest {
    pub protocol_id: u64,
    pub action: u32,
    pub transaction_id: u32,
}

impl UdpConnectRequest {
    pub fn new() -> Self {
        let transaction_id = rand::random();
        let protocol_id = 0x41727101980;
        Self {
            protocol_id,
            action: 0,
            transaction_id,
        }
    }

    pub fn as_bytes(&self) -> [u8; 16] {
        let mut bytes: [u8; 16] = [0; 16];
        let mut writer = bytes.writer();
        writer.write_all(&self.protocol_id.to_be_bytes()).unwrap();
        writer.write_all(&self.action.to_be_bytes()).unwrap();
        writer
            .write_all(&self.transaction_id.to_be_bytes())
            .unwrap();
        bytes
    }
}

impl Default for UdpConnectRequest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UdpConnectResponse {
    pub action: u32,
    pub transaction_id: u32,
    pub connection_id: u64,
}

impl UdpConnectResponse {
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let action: u32 = u32::from_be_bytes(bytes[0..4].try_into()?);
        let transaction_id: u32 = u32::from_be_bytes(bytes[4..8].try_into()?);
        let connection_id: u64 = u64::from_be_bytes(bytes[8..16].try_into()?);
        Ok(Self {
            action,
            transaction_id,
            connection_id,
        })
    }
}
