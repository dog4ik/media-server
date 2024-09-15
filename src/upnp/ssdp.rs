use std::{
    fmt::Display,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
    time::Duration,
};

use anyhow::Context;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::config;

use super::{router, urn};

const SSDP_IP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(SSDP_IP_ADDR, 1900));
const NOTIFY_INTERVAL_DURATION: Duration = Duration::from_secs(90);

const DATE_FORMAT: time::format_description::well_known::Rfc2822 =
    time::format_description::well_known::Rfc2822;

fn media_server_notify_message(local_addr: IpAddr) -> String {
    let port: config::Port = config::CONFIG.get_value();
    let location = format!(
        "http://{local_addr}:{port}{path}",
        port = port.0,
        path = router::DESC_PATH
    );
    let notify_message = NotifyAliveMessage {
        host: SSDP_ADDR,
        location: &location,
        usn: "my super unique usn: todo!",
        nt: NotificationType::RootDevice,
        nts: NotificationSubType::Alive,
        cache_control: 1800,
        server: "Linux/6.0 UPnP/2.0 MediaServer/1.0",
    };
    notify_message.to_string()
}

fn media_server_byebye_message() -> String {
    let notify_message = NotifyByeByeMessage {
        host: SSDP_ADDR,
        usn: "my unique usn: todo!",
        nt: NotificationType::RootDevice,
        nts: NotificationSubType::Alive,
    };
    notify_message.to_string()
}

pub async fn ssdp_listener() -> anyhow::Result<()> {
    let cancellation_token = CancellationToken::new();
    let local_ip = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1900);
    let socket = UdpSocket::bind(SocketAddr::V4(local_ip)).await.unwrap();
    //TODO: The TTL for the IP packet should default to 2 and should be configurable.
    socket
        .join_multicast_v4(SSDP_IP_ADDR, Ipv4Addr::UNSPECIFIED)
        .unwrap();
    let mut buf = [0; 2048];
    let mut notify_interval = tokio::time::interval(NOTIFY_INTERVAL_DURATION);
    notify_interval.tick().await;

    // NOTE: this feels wrong. Find the better solution
    let local_addr = {
        let socket =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).await?;
        socket.connect(SSDP_ADDR).await.unwrap();
        let local_addr = socket.local_addr().context("get local addr")?;
        tracing::debug!("Resolved local address {local_addr}");
        local_addr
    };

    let self_notify_init_message = media_server_notify_message(local_addr.ip());
    socket
        .send_to(self_notify_init_message.as_bytes(), SSDP_ADDR)
        .await
        .unwrap();

    loop {
        tokio::select! {
            Ok((read, sender)) = socket.recv_from(&mut buf) => {
                let data = &buf[..read];
                let Ok(string) = String::from_utf8(data.to_vec()) else {
                    tracing::warn!("Failed to construct string from {read} bytes");
                    continue;
                };
                match BroadcastMessage::parse_ssdp_payload(&string) {
                    Ok(BroadcastMessage::Search(msg)) => {
                        if let NotificationType::Urn(urn::URN{ urn_type: urn::UrnType::Device(device), version}) = msg.st {
                            println!("Recieved device search message: {:?}", device);
                        }
                        if let NotificationType::Urn(urn::URN{ urn_type: urn::UrnType::Service(service), version}) = msg.st {
                            println!("Recieved service search message: {:?}", service);
                        }
                        if let NotificationType::RootDevice = msg.st {
                            println!("Recieved root device search message: {:?}", msg.st);
                        }
                    }
                    Ok(msg) => {},
                    Err(e) => {
                        tracing::error!("Failed to parse broadcast message: {e}");
                        eprintln!("{}", string);
                    }
                };
            }
            _ = cancellation_token.cancelled() => {
                let self_byebye_message = media_server_byebye_message();
                socket.send_to(self_byebye_message.as_bytes(), SSDP_ADDR).await.unwrap();
            }
            _ = notify_interval.tick() => {
                // TODO: service notifications
                let self_notify_message = media_server_notify_message(local_addr.ip());
                socket.send_to(self_notify_message.as_bytes(), SSDP_ADDR).await.unwrap();
            }
            else => break
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum BroadcastMessage<'a> {
    Search(SearchMessage<'a>),
    NotifyAlive(NotifyAliveMessage<'a>),
    NotifyByeBye(NotifyByeByeMessage<'a>),
    NotifyUpdate(NotifyUpdateMessage<'a>),
}

#[derive(Debug, Copy, Clone)]
pub struct SearchMessage<'a> {
    /// For unicast requests, the field value shall be the domain name or IP address of the target device
    /// and either port 1900 or the SEARCHPORT provided by the target device.
    pub host: SocketAddr,
    pub man: &'a str,
    pub st: NotificationType<'a>,
    /// Field value contains maximum wait time in seconds. shall be greater than or equal to 1 and should
    /// be less than 5 inclusive. Device responses should be delayed a random duration between 0 and this many
    /// seconds to balance load for the control point when it processes responses. This value is allowed to be
    /// increased if a large number of devices are expected to respond
    pub mx: usize,
    /// Same as server in search messages
    pub user_agent: Option<&'a str>,
}

impl Display for SearchMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "M-SEARCH * HTTP/1.1
HOST: 239.255.255.250:1900
MAN: \"ssdp:discover\"
ST: {search_target}
MX: {mx}",
            search_target = self.st.to_string(),
            mx = self.mx
        )?;
        if let Some(user_agent) = self.user_agent {
            write!(f, "USER-AGENT: {user_agent}")?;
        }
        write!(f, "\r\n\r\n")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SearchResponse<'a> {
    cache_control: usize,
    location: &'a str,
    server: &'a str,
    st: NotificationType<'a>,
    usn: &'a str,
}

impl Display for SearchResponse<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "HTTP/1.1 200 OK
CACHE-CONTROL: max-age={cache_control}
LOCATION: {location}
SERVER: {server}
ST: {st}
USN: {usn}\r\n\r\n",
            cache_control = self.cache_control,
            location = self.location,
            server = self.server,
            st = self.st,
            usn = self.usn
        )
    }
}

impl<'a> TryFrom<&'a str> for SearchResponse<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NotificationType<'a> {
    /// `ssdp:all` A wildcard value that indicates the search is for all devices and services on the network. This is used to discover any UPnP device or service
    All,
    /// `upnp:rootdevice` A root device is a device that can be used to discover other UPnP devices and services.
    RootDevice,
    /// The UUID represents a unique identifier for a device or service.
    Uuid(&'a str),
    Urn(urn::URN<'a>),
}

impl<'a> TryFrom<&'a str> for NotificationType<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Ok(match value {
            "ssdp:all" => Self::All,
            "upnp:rootdevice" => Self::RootDevice,
            rest if rest.starts_with("urn:") => Self::Urn(urn::URN::try_from(rest)?),
            rest if rest.starts_with("uuid:") => {
                Self::Uuid(rest.strip_prefix("uuid:").expect("prefix to be uuid:"))
            }
            rest => Err(anyhow::anyhow!("Unknown notification type: {rest}"))?,
        })
    }
}

impl Display for NotificationType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotificationType::All => write!(f, "ssdp:all"),
            NotificationType::RootDevice => write!(f, "upnp:rootdevice"),
            NotificationType::Uuid(id) => write!(f, "uuid:{id}"),
            NotificationType::Urn(urn) => write!(f, "{urn}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Notification subtype. Specifies type of notification.
pub enum NotificationSubType {
    /// This is typically sent when a device is first powered on or joins the network, or to periodically reaffirm its presence
    Alive,
    /// Sent when a device is being removed from the network or shutting down.
    ByeBye,
    /// Used when there are changes in the device's details.
    Update,
}

impl Display for NotificationSubType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            NotificationSubType::Alive => "alive",
            NotificationSubType::ByeBye => "byebye",
            NotificationSubType::Update => "update",
        };
        write!(f, "ssdp:{msg}")
    }
}

impl FromStr for NotificationSubType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ssdp:alive" => Self::Alive,
            "ssdp:byebye" => Self::ByeBye,
            "ssdp:update" => Self::Update,
            rest => Err(anyhow::anyhow!("Unknown notification sub type: {rest}"))?,
        })
    }
}

#[derive(Debug, Copy, Clone)]
pub struct NotifyByeByeMessage<'a> {
    pub host: SocketAddr,
    /// The Unique Service Name, which combines a unique identifier (UUID) with the device or service type.
    /// This allows clients to uniquely identify the device or service instance
    pub usn: &'a str,
    /// Notification type. Specifies type of device/service.
    pub nt: NotificationType<'a>,
    /// Notification subtype. Specifies type of notification.
    pub nts: NotificationSubType,
}

impl Display for NotifyByeByeMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NOTIFY * HTTP/1.1
HOST: 239.255.255.250:1900
NT: {nt}
NTS: {nts}
USN: {usn}\r\n\r\n",
            nt = self.nt,
            nts = self.nts,
            usn = self.usn,
        )
    }
}

#[derive(Debug, Copy, Clone)]
pub struct NotifyUpdateMessage<'a> {
    pub host: SocketAddr,
    /// The Unique Service Name, which combines a unique identifier (UUID) with the device or service type.
    /// This allows clients to uniquely identify the device or service instance
    pub usn: &'a str,
    /// Url of device description
    pub location: &'a str,
    /// Notification type. Specifies type of device/service.
    pub nt: NotificationType<'a>,
    /// Notification subtype. Specifies type of notification.
    pub nts: NotificationSubType,
}

impl Display for NotifyUpdateMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NOTIFY * HTTP/1.1
HOST: 239.255.255.250:1900
LOCATION: {location}
NT: {nt}
NTS: {nts}
USN: {usn}\r\n\r\n",
            location = self.location,
            nt = self.nt,
            nts = self.nts,
            usn = self.usn,
        )
    }
}

#[derive(Debug, Copy, Clone)]
pub struct NotifyAliveMessage<'a> {
    pub host: SocketAddr,
    /// Url of device description
    pub location: &'a str,
    /// The Unique Service Name, which combines a unique identifier (UUID) with the device or service type.
    /// This allows clients to uniquely identify the device or service instance
    pub usn: &'a str,
    /// Notification type. Specifies type of device/service.
    pub nt: NotificationType<'a>,
    /// Notification subtype. Specifies type of notification.
    pub nts: NotificationSubType,
    /// Cache life time in seconds
    pub cache_control: usize,
    /// Information about the software used by the origin server to handle the request
    pub server: &'a str,
}

impl Display for NotifyAliveMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NOTIFY * HTTP/1.1
HOST: 239.255.255.250:1900
CACHE-CONTROL: max-age={cache_control}
LOCATION: {location}
NT: {nt}
NTS: {nts}
SERVER: {server}
USN: {usn}\r\n\r\n",
            cache_control = self.cache_control,
            location = self.location,
            nt = self.nt,
            nts = self.nts,
            server = self.server,
            usn = self.usn,
        )
    }
}

impl BroadcastMessage<'_> {
    pub fn parse_ssdp_payload(s: &str) -> anyhow::Result<BroadcastMessage<'_>> {
        let mut lines = s.lines();
        let request_line = lines.next().context("request line")?;
        let (method, _) = request_line.split_once(' ').context("split request line")?;
        let headers = lines.filter_map(|l| l.split_once(": "));
        match method {
            "M-SEARCH" => {
                let mut host = None;
                let mut man = None;
                let mut st = None;
                let mut mx = None;
                let mut user_agent = None;
                for (name, value) in headers {
                    let value = value.trim();
                    match name.to_ascii_lowercase().as_str() {
                        "host" => {
                            host = Some(SocketAddr::V4(
                                SocketAddrV4::from_str(value).context("parse host address")?,
                            ));
                        }
                        "man" => man = Some(value),
                        "st" => st = Some(NotificationType::try_from(value)?),
                        "mx" => mx = Some(value.parse()?),
                        "user-agent" => user_agent = Some(value),
                        _ => (),
                    }
                }
                let host = host.context("missing host")?;
                let man = man.context("missing man")?;
                let st = st.context("missing st")?;
                let mx = mx.context("missing mx")?;
                let search_message = SearchMessage {
                    host,
                    man,
                    st,
                    mx,
                    user_agent,
                };
                Ok(BroadcastMessage::Search(search_message))
            }
            "NOTIFY" => {
                let mut host = None;
                let mut nts = None;
                let mut location = None;
                let mut nt = None;
                let mut usn = None;
                let mut cache_control = None;
                let mut server = None;
                for (name, value) in headers {
                    let value = value.trim();
                    match name.to_ascii_lowercase().as_str() {
                        "host" => {
                            host = Some(SocketAddr::V4(
                                SocketAddrV4::from_str(value).context("parse host address")?,
                            ));
                        }
                        "location" => location = Some(value),
                        "usn" => usn = Some(value),
                        "nt" => nt = Some(NotificationType::try_from(value)?),
                        "nts" => nts = Some(NotificationSubType::from_str(value)?),
                        "server" => server = Some(value),
                        "cache-control" => {
                            let (prefix, cache_duration) =
                                value.split_once('=').context("split cache control")?;
                            anyhow::ensure!(prefix.trim() == "max-age");
                            cache_control =
                                Some(cache_duration.parse().context("parse duration seconds")?)
                        }
                        _ => (),
                    }
                }
                let nt = nt.context("missing nt")?;
                let nts = nts.context("missing nts")?;
                let host = host.context("missing host")?;
                let usn = usn.context("missing usn")?;
                match nts {
                    NotificationSubType::Alive => {
                        let location = location.context("missing location")?;
                        let cache_control = cache_control.context("missing cache control")?;
                        let server = server.context("missing server")?;
                        let notify_message = NotifyAliveMessage {
                            host,
                            location,
                            usn,
                            nt,
                            nts,
                            cache_control,
                            server,
                        };
                        Ok(BroadcastMessage::NotifyAlive(notify_message))
                    }
                    NotificationSubType::ByeBye => {
                        let byebye_message = NotifyByeByeMessage { host, usn, nt, nts };
                        Ok(BroadcastMessage::NotifyByeBye(byebye_message))
                    }
                    NotificationSubType::Update => {
                        let location = location.context("missing location")?;
                        let update_message = NotifyUpdateMessage {
                            location,
                            host,
                            usn,
                            nt,
                            nts,
                        };
                        Ok(BroadcastMessage::NotifyUpdate(update_message))
                    }
                }
            }
            rest => Err(anyhow::anyhow!("Unknown method encountered: {rest}")),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::BroadcastMessage;

    #[test]
    fn prase_broadcast_message() {
        let notify = r#"NOTIFY * HTTP/1.1
HOST: 239.255.255.250:1900
CACHE-CONTROL: max-age=1800
LOCATION: http://192.168.1.1:49152/IGDdevicedesc.xml
OPT: "http://schemas.upnp.org/upnp/1/0/"; ns=01
01-NLS: 2c118d74-1dd2-11b2-888a-b21a12907e76
NT: urn:schemas-upnp-org:service:WANEthernetLinkConfig:1
NTS: ssdp:alive
SERVER: Linux/3.14.77, UPnP/1.0, Portable SDK for UPnP devices/1.6.19
X-User-Agent: redsonic
USN: uuid:ebf5a0a0-1dd1-11b2-a92f-e89f80eb7241::urn:schemas-upnp-org:service:WANEthernetLinkConfig:1"#;

        let notify_message = BroadcastMessage::parse_ssdp_payload(notify).unwrap();
        assert!(matches!(
            notify_message,
            BroadcastMessage::NotifyAlive { .. }
        ));
        let m_search = r#"M-SEARCH * HTTP/1.1
HOST: 239.255.255.250:1900
MAN: "ssdp:discover"
MX: 1
ST: urn:dial-multiscreen-org:service:dial:1
USER-AGENT: Microsoft Edge/128.0.2739.67 Windows"#;
        let m_search_message = BroadcastMessage::parse_ssdp_payload(m_search).unwrap();
        assert!(matches!(m_search_message, BroadcastMessage::Search { .. }));
    }
}
