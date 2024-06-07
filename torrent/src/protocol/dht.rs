//NOTE: dont forget to add dht capability handshake flag when its done
use std::{collections::HashMap, net::SocketAddr, ops::Range, time::Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
enum NodeStatus {
    Unknown,
    Good,
    Questionable,
    Bad,
}

#[derive(Debug, Clone)]
pub struct DHTNode {
    node_id: [u8; 20],
    addr: SocketAddr,
    status: NodeStatus,
}

#[derive(Debug, Clone)]
pub struct DHTClient {
    id: [u8; 20],
    info_hash: [u8; 20],
    addr: SocketAddr,
    routing_table: HashMap<[u8; 20], SocketAddr>,
}

#[derive(Debug, Clone)]
pub struct Bucket {
    range: Range<[u8; 20]>,
    last_changed: Instant,
    nodes: Vec<DHTNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KRPCMessage {
    #[serde(rename = "t")]
    transaction_id: String,
    #[serde(rename = "y")]
    message_type: String,
    #[serde(flatten)]
    payload: KRPCPayload,
    #[serde(rename = "v")]
    client_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KRPCPayload {
    Query {
        #[serde(rename = "q")]
        query: String,
        #[serde(rename = "a")]
        arguments: DHTQuery,
    },
    Response {
        #[serde(rename = "r")]
        response: DHTResponse,
    },
    Error {
        #[serde(rename = "e")]
        error: (usize, String),
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum DHTQuery {
    AnnouncePeer {
        id: String,
        implied_port: Option<usize>,
        info_hash: String,
        port: u16,
        token: String,
    },
    FindNode {
        target: String,
        id: String,
    },
    GetPeers {
        id: String,
        info_hash: String,
    },
    Ping {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum DHTResponse {
    FindNode {
        id: String,
        nodes: String,
    },
    /// Ping and announce responses have the same signature thus they are indistinguishable
    PingOrAnnounce {
        id: String,
    },
    GetPeers {
        id: String,
        token: String,
        #[serde(flatten)]
        values: DHTGetPeersResponseValue,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DHTGetPeersResponseValue {
    Values(Vec<String>),
    Nodes(String),
}

impl DHTClient {
    pub fn new(addr: SocketAddr, info_hash: [u8; 20]) -> Self {
        Self {
            addr,
            info_hash,
            id: rand::random(),
            routing_table: HashMap::new(),
        }
    }

    pub fn closest_node(&self) -> Option<&SocketAddr> {
        self.routing_table
            .iter()
            .min_by_key(|(x, _)| distance(&self.info_hash, *x))
            .map(|(_, addr)| addr)
    }
}

fn distance(from: &[u8; 20], to: &[u8; 20]) -> [u8; 20] {
    let xor_result: Vec<u8> = from
        .iter()
        .zip(to.iter())
        .map(|(b1, b2)| b1 ^ b2)
        .collect();
    xor_result.try_into().unwrap()
}

#[cfg(test)]
mod tests {

    use std::assert_matches::assert_matches;

    use crate::protocol::dht::{
        DHTGetPeersResponseValue, DHTQuery, DHTResponse, KRPCMessage, KRPCPayload,
    };

    #[test]
    fn dht_parse_error_message() {
        let raw_err_response = r#"d1:eli201e23:A Generic Error Ocurrede1:t2:aa1:y1:ee"#;
        let err_response: KRPCMessage = serde_bencode::from_str(&raw_err_response).unwrap();
        assert_matches!(
            err_response.payload,
            KRPCPayload::Error {
                error: (status, msg)
            } if status == 201 && msg == "A Generic Error Ocurred"
        );
    }

    #[test]
    fn dht_parse_ping_message() {
        let raw_request = r#"d1:ad2:id20:abcdefghij0123456789e1:q4:ping1:t2:aa1:y1:qe"#;
        let request: KRPCMessage = serde_bencode::from_str(&raw_request).unwrap();
        assert_matches!(
            request.payload,
            KRPCPayload::Query {
                arguments: DHTQuery::Ping { id },
                query,
            } if id == "abcdefghij0123456789" && query == "ping"
        );

        let raw_response = r#"d1:rd2:id20:mnopqrstuvwxyz123456e1:t2:aa1:y1:re"#;
        let rensponse: KRPCMessage = serde_bencode::from_str(&raw_response).unwrap();
        assert_matches!(
            rensponse.payload,
            KRPCPayload::Response {
                response: DHTResponse::PingOrAnnounce { id }
            } if id == "mnopqrstuvwxyz123456"
        );
    }

    #[test]
    fn dht_parse_find_node_message() {
        let raw_request = r#"d1:ad2:id20:abcdefghij01234567896:target20:mnopqrstuvwxyz123456e1:q9:find_node1:t2:aa1:y1:qe"#;
        let request: KRPCMessage = serde_bencode::from_str(&raw_request).unwrap();
        assert_matches!(
            request.payload,
            KRPCPayload::Query {
                arguments: DHTQuery::FindNode { id, target },
                query,
            } if id == "abcdefghij0123456789" && target == "mnopqrstuvwxyz123456" && query == "find_node"
        );

        let raw_response = r#"d1:rd2:id20:0123456789abcdefghij5:nodes9:def456...e1:t2:aa1:y1:re"#;
        let rensponse: KRPCMessage = serde_bencode::from_str(&raw_response).unwrap();
        assert_matches!(
            rensponse.payload,
            KRPCPayload::Response {
                response: DHTResponse::FindNode { id, nodes }
            } if id == "0123456789abcdefghij" && nodes == "def456..."
        );
    }

    #[test]
    fn dht_parse_get_peers_message() {
        let raw_request = r#"d1:ad2:id20:abcdefghij01234567899:info_hash20:mnopqrstuvwxyz123456e1:q9:get_peers1:t2:aa1:y1:qe"#;
        let request: KRPCMessage = serde_bencode::from_str(&raw_request).unwrap();
        assert_matches!(
            request.payload,
            KRPCPayload::Query {
                arguments: DHTQuery::GetPeers { info_hash, id },
                query,
            } if id == "abcdefghij0123456789" && info_hash == "mnopqrstuvwxyz123456" && query == "get_peers"
        );

        let raw_response_with_peers = r#"d1:rd2:id20:abcdefghij01234567895:token8:aoeusnth6:valuesl6:axje.u6:idhtnmee1:t2:aa1:y1:re"#;
        let rensponse_with_peers: KRPCMessage =
            serde_bencode::from_str(&raw_response_with_peers).unwrap();
        assert_matches!(
            rensponse_with_peers.payload,
            KRPCPayload::Response {
                response: DHTResponse::GetPeers { id, token, values: DHTGetPeersResponseValue::Values(values) }
            } if id == "abcdefghij0123456789" && token == "aoeusnth" && values == vec!["axje.u", "idhtnm"]
        );

        let raw_response_with_nodes =
            r#"d1:rd2:id20:abcdefghij01234567895:nodes9:def456...5:token8:aoeusnthe1:t2:aa1:y1:re"#;
        let rensponse_with_nodes: KRPCMessage =
            serde_bencode::from_str(&raw_response_with_nodes).unwrap();
        assert_matches!(
            rensponse_with_nodes.payload,
            KRPCPayload::Response {
                response: DHTResponse::GetPeers { id, token, values: DHTGetPeersResponseValue::Nodes(nodes) }
            } if id == "abcdefghij0123456789" && token == "aoeusnth" && nodes == "def456..."
        );
    }

    #[test]
    fn dht_parse_announce_peer_message() {
        let raw_request = r#"d1:ad2:id20:abcdefghij012345678912:implied_porti1e9:info_hash20:mnopqrstuvwxyz1234564:porti6881e5:token8:aoeusnthe1:q13:announce_peer1:t2:aa1:y1:qe"#;
        let request: KRPCMessage = serde_bencode::from_str(&raw_request).unwrap();
        assert_matches!(
            request.payload,
            KRPCPayload::Query {
                arguments: DHTQuery::AnnouncePeer { id, info_hash, port, token, implied_port },
                query,
            } if
        id == "abcdefghij0123456789" &&
        implied_port == Some(1) &&
        info_hash == "mnopqrstuvwxyz123456" &&
        port == 6881 &&
        token == "aoeusnth" &&
        query == "announce_peer"
        );

        let raw_response = r#"d1:rd2:id20:mnopqrstuvwxyz123456e1:t2:aa1:y1:re"#;
        let rensponse: KRPCMessage = serde_bencode::from_str(&raw_response).unwrap();
        assert_matches!(
            rensponse.payload,
            KRPCPayload::Response {
                response: DHTResponse::PingOrAnnounce { id }
            } if id == "mnopqrstuvwxyz123456"
        );
    }
}
