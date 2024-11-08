use bytes::{Bytes, BytesMut};
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize};
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
};

use super::extension::Extension;

#[derive(Debug, Clone)]
pub struct PexHistory {
    history: Vec<PexHistoryEntry>,
}

#[derive(Debug, Clone, Copy)]
pub struct PexHistoryEntry {
    pub addr: SocketAddr,
    pub is_added: bool,
}

impl PexHistoryEntry {
    pub fn added(addr: SocketAddr) -> Self {
        Self {
            addr,
            is_added: true,
        }
    }
    pub fn dropped(addr: SocketAddr) -> Self {
        Self {
            addr,
            is_added: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PexFlags(pub u8);

impl PexFlags {
    const PREFER_ENCRYPTION: u8 = 0x01;
    const SEED_ONLY: u8 = 0x02;
    const SUPPORTS_UTP: u8 = 0x04;
    const SUPPORTS_HOLEPUNCH: u8 = 0x08;
    const REACHABLE: u8 = 0x10;

    fn set_field(&mut self, field: u8, force: bool) {
        if force {
            self.0 |= field;
        } else {
            self.0 &= !field;
        }
    }
    /// prefers encryption, as indicated by e field in extension handshake
    pub fn prefer_encryption(&self) -> bool {
        self.0 & Self::PREFER_ENCRYPTION != 0
    }
    pub fn set_prefer_encryption(&mut self, force: bool) {
        self.set_field(Self::PREFER_ENCRYPTION, force)
    }

    /// seed/upload_only
    pub fn seed_only(&self) -> bool {
        self.0 & Self::SEED_ONLY != 0
    }
    pub fn set_seed_only(&mut self, force: bool) {
        self.set_field(Self::SEED_ONLY, force)
    }

    /// supports uTP
    pub fn supports_utp(&self) -> bool {
        self.0 & Self::SUPPORTS_UTP != 0
    }
    pub fn set_supports_utp(&mut self, force: bool) {
        self.set_field(Self::SUPPORTS_UTP, force)
    }

    /// peer indicated ut_holepunch support in extension handshake
    pub fn supports_holepunch(&self) -> bool {
        self.0 & Self::SUPPORTS_HOLEPUNCH != 0
    }
    pub fn set_supports_holepunch(&mut self, force: bool) {
        self.set_field(Self::SUPPORTS_HOLEPUNCH, force)
    }

    /// outgoing connection, peer is reachable
    pub fn reachable(&self) -> bool {
        self.0 & Self::REACHABLE != 0
    }
    pub fn set_reachable(&mut self, force: bool) {
        self.set_field(Self::REACHABLE, force)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PexEntry {
    pub addr: SocketAddr,
    pub flags: Option<PexFlags>,
}

impl PexEntry {
    pub fn new(addr: SocketAddr, flags: Option<PexFlags>) -> Self {
        Self { addr, flags }
    }
}

#[derive(Debug, Clone)]
pub struct PexMessage {
    pub added: Vec<PexEntry>,
    pub dropped: Vec<SocketAddr>,
}

impl PexMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_bencode::Error> {
        serde_bencode::from_bytes(bytes)
    }
    pub fn as_bytes(&self) -> Vec<u8> {
        serde_bencode::to_bytes(self).unwrap()
    }
}

struct UtMessageVisitor;

impl<'v> Visitor<'v> for UtMessageVisitor {
    type Value = PexMessage;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "bencoded map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'v>,
    {
        let mut added: Option<Bytes> = None;
        let mut added_flags: Option<Bytes> = None;
        let mut added6: Option<Bytes> = None;
        let mut added6_flags: Option<Bytes> = None;
        let mut dropped: Option<Bytes> = None;
        let mut dropped6: Option<Bytes> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_ref() {
                "added" => added = Some(map.next_value()?),
                "added.f" => added_flags = Some(map.next_value()?),
                "added6" => added6 = Some(map.next_value()?),
                "added6.f" => added6_flags = Some(map.next_value()?),
                "dropped" => dropped = Some(map.next_value()?),
                "dropped6" => dropped6 = Some(map.next_value()?),
                _ => {
                    return Err(serde::de::Error::unknown_variant(
                        &key,
                        &[
                            "added", "added.f", "added6", "added6.f", "dropped", "dropped6",
                        ],
                    ))
                }
            };
        }
        if added.is_none() && added6.is_none() && dropped.is_none() && dropped6.is_none() {
            return Err(serde::de::Error::missing_field("Messages must contain at least one of the following fields: added, added6, dropped, dropped6"));
        }

        let parse_ipv4 = |chunk: [u8; 6]| {
            let addr = u32::from_be_bytes(chunk[..4].try_into().unwrap());
            let port = u16::from_be_bytes(chunk[4..].try_into().unwrap());
            let ip = Ipv4Addr::from_bits(addr);
            SocketAddr::V4(SocketAddrV4::new(ip, port))
        };

        let parse_ipv6 = |chunk: [u8; 18]| {
            let addr = u128::from_be_bytes(chunk[..16].try_into().unwrap());
            let port = u16::from_be_bytes(chunk[16..].try_into().unwrap());
            let ip = Ipv6Addr::from_bits(addr);
            SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0))
        };

        let mut added_list = Vec::with_capacity(
            added.as_ref().map(|a| a.len() / 6).unwrap_or_default()
                + added6.as_ref().map(|x| x.len() / 18).unwrap_or_default(),
        );

        if let Some(added) = added {
            for (i, chunk) in added.array_chunks::<6>().enumerate() {
                let flags = added_flags
                    .as_ref()
                    .and_then(|f| f.get(i))
                    .map(|x| PexFlags(*x));
                added_list.push(PexEntry {
                    addr: parse_ipv4(*chunk),
                    flags,
                });
            }
        }

        if let Some(added6) = added6 {
            for (i, chunk) in added6.array_chunks::<18>().enumerate() {
                let flags = added6_flags
                    .as_ref()
                    .and_then(|f| f.get(i))
                    .map(|x| PexFlags(*x));
                added_list.push(PexEntry {
                    addr: parse_ipv6(*chunk),
                    flags,
                });
            }
        }

        let mut dropped_list = Vec::with_capacity(
            dropped.as_ref().map(|a| a.len() / 6).unwrap_or_default()
                + dropped6.as_ref().map(|x| x.len() / 18).unwrap_or_default(),
        );

        if let Some(dropped) = dropped {
            for chunk in dropped.array_chunks::<6>() {
                dropped_list.push(parse_ipv4(*chunk));
            }
        }

        if let Some(dropped6) = dropped6 {
            for chunk in dropped6.array_chunks::<18>() {
                dropped_list.push(parse_ipv6(*chunk));
            }
        }

        Ok(PexMessage {
            added: added_list,
            dropped: dropped_list,
        })
    }
}

impl<'de> Deserialize<'de> for PexMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(UtMessageVisitor)
    }
}

impl Serialize for PexMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut added = BytesMut::new();
        let mut added_flags = BytesMut::new();
        let mut added6 = BytesMut::new();
        let mut added6_flags = BytesMut::new();
        let mut dropped = BytesMut::new();
        let mut dropped6 = BytesMut::new();
        for entry in &self.added {
            match entry.addr {
                SocketAddr::V4(addr) => {
                    added.extend(addr.ip().to_bits().to_be_bytes());
                    added.extend(addr.port().to_be_bytes());
                    if let Some(flags) = entry.flags {
                        added_flags.extend([flags.0])
                    }
                }
                SocketAddr::V6(addr) => {
                    added6.extend(addr.ip().to_bits().to_be_bytes());
                    added6.extend(addr.port().to_be_bytes());
                    if let Some(flags) = entry.flags {
                        added6_flags.extend([flags.0])
                    }
                }
            }
        }
        for entry in &self.dropped {
            match entry {
                SocketAddr::V4(addr) => {
                    dropped.extend(addr.ip().to_bits().to_be_bytes());
                    dropped.extend(addr.port().to_be_bytes());
                }
                SocketAddr::V6(addr) => {
                    dropped6.extend(addr.ip().to_bits().to_be_bytes());
                    dropped6.extend(addr.port().to_be_bytes());
                }
            }
        }
        let size_hint: usize = !added.is_empty() as usize
            + !added_flags.is_empty() as usize
            + !added6.is_empty() as usize
            + !added6_flags.is_empty() as usize
            + !dropped.is_empty() as usize
            + !dropped6.is_empty() as usize;
        let mut map = serializer.serialize_map(Some(size_hint))?;
        if !added.is_empty() {
            map.serialize_entry("added", &added)?;
        }
        if !added_flags.is_empty() {
            map.serialize_entry("added.f", &added_flags)?;
        }
        if !added6.is_empty() {
            map.serialize_entry("added6", &added6)?;
        }
        if !added6_flags.is_empty() {
            map.serialize_entry("added6.f", &added6_flags)?;
        }
        if !dropped.is_empty() {
            map.serialize_entry("dropped", &dropped)?;
        }
        if !dropped6.is_empty() {
            map.serialize_entry("dropped6", &dropped6)?;
        }
        map.end()
    }
}

impl PexHistory {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    pub fn push_value(&mut self, value: PexHistoryEntry) {
        self.history.push(value);
    }

    /// Latest point in history
    pub fn tip(&self) -> usize {
        self.history.len()
    }

    pub fn pex_message(&self, offset: usize) -> PexMessage {
        let relevant_history = &self.history[offset..];
        let mut added_set = HashSet::new();
        let mut dropped_set = HashSet::new();
        for entry in relevant_history {
            if entry.is_added {
                added_set.insert(entry.addr);
                dropped_set.remove(&entry.addr);
            } else {
                added_set.remove(&entry.addr);
                dropped_set.insert(entry.addr);
            }
        }
        PexMessage {
            added: added_set
                .into_iter()
                .map(|ip| PexEntry {
                    addr: ip,
                    flags: None,
                })
                .collect(),
            dropped: dropped_set.into_iter().collect(),
        }
    }
}

impl Default for PexHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl From<PexMessage> for bytes::Bytes {
    fn from(value: PexMessage) -> Self {
        serde_bencode::to_bytes(&value)
            .expect("serialization infallible")
            .into()
    }
}

impl TryFrom<&[u8]> for PexMessage {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let pex_message = serde_bencode::from_bytes(value)?;
        Ok(pex_message)
    }
}

impl Extension<'_> for PexMessage {
    const NAME: &'static str = "ut_pex";
    const CLIENT_ID: u8 = 2;
}

#[derive(Debug)]
pub struct PexPeers {
    /// Map from suggested ip to peers that suggested it.
    pub peer_map: BTreeMap<SocketAddr, BTreeSet<IpAddr>>,
}

impl PexPeers {
    pub fn add_peer(&mut self, from: SocketAddr, peer: SocketAddr) {
        let entry = self.peer_map.entry(peer);
        match entry {
            Entry::Vacant(vacant) => {
                let mut set = BTreeSet::new();
                set.insert(from.ip());
                vacant.insert(set);
            }
            Entry::Occupied(mut occupied) => {
                occupied.get_mut().insert(from.ip());
            }
        };
    }

    pub fn remove_peer(&mut self, from: SocketAddr, peer: SocketAddr) {
        if let Some(set) = self.peer_map.get_mut(&peer) {
            set.remove(&from.ip());
        }
    }

    pub fn pop_best(&mut self) -> Option<SocketAddr> {
        let mut max_val = 0;
        let mut best_peer = None;
        for (key, val) in &self.peer_map {
            if val.len() > max_val {
                max_val = val.len();
                best_peer = Some(*key)
            }
        }
        if let Some(best_peer) = best_peer {
            self.peer_map.remove(&best_peer);
        }
        best_peer
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    use crate::protocol::pex::PexEntry;

    use super::PexMessage;

    #[test]
    fn reencode_pex_message() {
        let ip = Ipv4Addr::LOCALHOST;
        let addr = SocketAddr::V4(SocketAddrV4::new(ip, 1828));
        let pex_message = PexMessage {
            added: vec![PexEntry::new(addr, None)],
            dropped: vec![addr, addr],
        };
        let encoded = pex_message.as_bytes();
        let decoded = PexMessage::from_bytes(&encoded).unwrap();
        assert_eq!(pex_message.dropped, decoded.dropped);
        assert_eq!(pex_message.added, decoded.added);
    }
}
