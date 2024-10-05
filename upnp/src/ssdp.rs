use core::str;
use std::{
    borrow::Cow,
    fmt::Display,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    ops::Range,
    str::FromStr,
    time::Duration,
};

use anyhow::Context;
use rand::Rng;
use socket2::{Domain, Protocol, Type};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::config;

use super::{
    device_description::{self, UDN},
    router, urn, SERVER_UUID,
};

const SSDP_IP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(SSDP_IP_ADDR, 1900));
const NOTIFY_INTERVAL_DURATION: Duration = Duration::from_secs(90);

const DATE_FORMAT: time::format_description::well_known::Rfc2822 =
    time::format_description::well_known::Rfc2822;

const CACHE_CONTROL: usize = 1800;

// TODO: Real values please
const SERVER: &str = "Linux/6.10.10-arch1-1 UPnP/2.0 MediaServer/1.0";

async fn sleep_rand_millis_duration(range: &Range<u64>) {
    let range = {
        let mut rng = rand::thread_rng();
        rng.gen_range(range.clone())
    };
    tokio::time::sleep(Duration::from_millis(range)).await;
}

fn bind_ssdp_socket(ttl: Option<u32>) -> anyhow::Result<UdpSocket> {
    let local_ip = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1900);
    let socket = socket2::Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_ttl(ttl.unwrap_or(2))?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.join_multicast_v4(&SSDP_IP_ADDR, &Ipv4Addr::UNSPECIFIED)?;
    socket.bind(&SocketAddr::V4(local_ip).into())?;
    let socket = UdpSocket::from_std(socket.into())?;
    Ok(socket)
}

async fn resolve_local_addr() -> anyhow::Result<SocketAddr> {
    // NOTE: this feels wrong. Find the better solution
    let socket =
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).await?;
    socket.connect(SSDP_ADDR).await?;
    socket.local_addr().context("get local addr")
}

#[derive(Debug)]
pub struct SsdpListener {
    socket: UdpSocket,
    local_addr: SocketAddr,
    boot_id: usize,
    location: String,
    unicast_port: u16,
}

impl SsdpListener {
    pub async fn bind(ttl: Option<u32>) -> anyhow::Result<Self> {
        let socket = bind_ssdp_socket(ttl).context("failed to bind ssdp socket")?;
        let local_addr = resolve_local_addr().await?;
        tracing::debug!("Resolved local ip address {local_addr}");
        let port: config::Port = config::CONFIG.get_value();
        let location = format!(
            "http://{addr}:{port}/upnp{path}",
            addr = local_addr.ip(),
            port = port.0,
            path = router::DESC_PATH
        );

        Ok(Self {
            socket,
            local_addr,
            boot_id: 0,
            location,
            unicast_port: 18398,
        })
    }

    pub async fn announce(&mut self, receiver: SocketAddr) -> anyhow::Result<()> {
        let sleep_range = 0..100;
        let local_ip = self.local_addr.ip();
        let first = NotifyAliveMessage::first_server_message(local_ip);
        self.socket
            .send_to(first.to_string().as_bytes(), &receiver)
            .await?;
        sleep_rand_millis_duration(&sleep_range).await;

        let second = NotifyAliveMessage::second_server_message(local_ip);
        self.socket
            .send_to(second.to_string().as_bytes(), &receiver)
            .await?;
        sleep_rand_millis_duration(&sleep_range).await;

        let third = NotifyAliveMessage::third_server_message(local_ip);
        self.socket
            .send_to(third.to_string().as_bytes(), &receiver)
            .await?;
        sleep_rand_millis_duration(&sleep_range).await;

        let urn = urn::URN {
            version: 1,
            urn_type: urn::UrnType::Service(urn::ServiceType::ContentDirectory),
        };
        let content_directory = NotifyAliveMessage::service_message(local_ip, urn);
        self.socket
            .send_to(content_directory.to_string().as_bytes(), &receiver)
            .await?;
        self.boot_id += 1;
        tracing::debug!("Finished announcing media server to: {receiver}");
        Ok(())
    }

    pub async fn musticast_search_response(
        &mut self,
        sender: SocketAddr,
        msg: SearchMessage<'_>,
    ) -> anyhow::Result<()> {
        let sleep_range = 0..(msg.mx as u64).min(5) * 1000;
        sleep_rand_millis_duration(&sleep_range).await;
        let search_response = SearchResponse {
            cache_control: 1800,
            location: &self.location,
            server: SERVER,
            st: msg.st,
            usn: USN::device_uuid(UDN::new(SERVER_UUID)),
            boot_id: self.boot_id,
            config_id: 1,
            search_port: self.unicast_port,
        }
        .to_string();
        self.socket
            .send_to(search_response.as_bytes(), sender)
            .await?;
        tracing::debug!("Responded to multicast search from {sender}");
        Ok(())
    }

    pub async fn listen(&mut self, cancellation_token: CancellationToken) -> anyhow::Result<()> {
        self.announce(SSDP_ADDR).await?;

        let mut notify_interval = tokio::time::interval(NOTIFY_INTERVAL_DURATION);
        notify_interval.tick().await;

        let mut buf = [0; 2048];
        loop {
            tokio::select! {
                Ok((read, sender)) = self.socket.recv_from(&mut buf) => {
                    let data = &buf[..read];
                    // TODO: this will block everything because it sleeps. We must be able to
                    // respond to others meanwhile.
                    if let Err(e) = self.handle_message(data, sender).await {
                        tracing::warn!("failed to handle ssdp message: {e}");
                    };
                }
                _ = cancellation_token.cancelled() => {
                    self.handle_shutdown().await?;
                    return Ok(())
                }
                _ = notify_interval.tick() => {
                    self.announce(SSDP_ADDR).await?;
                }
            }
        }
    }

    async fn handle_message(&mut self, data: &[u8], sender: SocketAddr) -> anyhow::Result<()> {
        let payload = str::from_utf8(data).context("construct string from bytes")?;
        let message =
            BroadcastMessage::parse_ssdp_payload(payload).context("parse ssdp message")?;
        match message {
            BroadcastMessage::Search(msg) => match msg.st {
                NotificationType::All => {
                    self.musticast_search_response(sender, msg).await?;
                }
                NotificationType::RootDevice => todo!(),
                NotificationType::Uuid(uuid) if uuid == SERVER_UUID => {
                    self.musticast_search_response(sender, msg).await?;
                }
                NotificationType::Uuid(_) => {}
                NotificationType::Urn(ref urn) => match urn.urn_type {
                    urn::UrnType::Device(urn::DeviceType::MediaServer) => {
                        self.musticast_search_response(sender, msg).await?;
                    }
                    urn::UrnType::Service(urn::ServiceType::ContentDirectory) => {
                        self.musticast_search_response(sender, msg).await?;
                    }
                    _ => {}
                },
            },
            BroadcastMessage::NotifyAlive(msg) => {
                println!("Recieved alive message from: {}", msg.server);
            }
            BroadcastMessage::NotifyByeBye(msg) => {
                println!("Recieved byebye message from: {}", msg.usn);
            }
            BroadcastMessage::NotifyUpdate(msg) => {
                println!("Recieved update message from: {}", msg.usn);
            }
        }
        Ok(())
    }

    async fn handle_shutdown(&self) -> anyhow::Result<()> {
        let self_byebye_message = NotifyByeByeMessage::media_server();
        tracing::info!("Sending bye bye message");
        self.socket
            .send_to(self_byebye_message.to_string().as_bytes(), SSDP_ADDR)
            .await?;
        Ok(())
    }
}

///  Unique Service Name. Identifies a unique instance of a device or service.
#[derive(Debug, Clone)]
pub struct USN {
    udn: device_description::UDN,
    kind: USNkind,
}

#[derive(Debug, Clone)]
pub enum USNkind {
    RootDevice,
    DeviceUuid,
    URN(urn::URN),
}

impl USN {
    pub const fn root_device(udn: device_description::UDN) -> Self {
        Self {
            udn,
            kind: USNkind::RootDevice,
        }
    }
    pub const fn device_uuid(udn: device_description::UDN) -> Self {
        Self {
            udn,
            kind: USNkind::DeviceUuid,
        }
    }
    pub const fn urn(udn: device_description::UDN, urn: urn::URN) -> Self {
        Self {
            udn,
            kind: USNkind::URN(urn),
        }
    }
}

impl Display for USN {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.udn)?;
        match &self.kind {
            USNkind::RootDevice => write!(f, "::upnp:rootdevice"),
            USNkind::DeviceUuid => Ok(()),
            USNkind::URN(urn) => write!(f, "::{urn}"),
        }
    }
}

impl FromStr for USN {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((start, rest)) = s.split_once("::") else {
            let udn = device_description::UDN::from_str(s)?;
            return Ok(Self::device_uuid(udn));
        };
        let udn = device_description::UDN::from_str(start)?;

        if rest == "upnp:rootdevice" {
            return Ok(Self::root_device(udn));
        }

        let urn = urn::URN::from_str(rest)?;
        Ok(Self {
            udn,
            kind: USNkind::URN(urn),
        })
    }
}

#[derive(Debug)]
pub enum BroadcastMessage<'a> {
    Search(SearchMessage<'a>),
    NotifyAlive(NotifyAliveMessage<'a>),
    NotifyByeBye(NotifyByeByeMessage),
    NotifyUpdate(NotifyUpdateMessage<'a>),
}

#[derive(Debug, Clone)]
pub struct SearchMessage<'a> {
    /// For unicast requests, the field value shall be the domain name or IP address of the target device
    /// and either port 1900 or the SEARCHPORT provided by the target device.
    pub host: SocketAddr,
    pub man: &'a str,
    pub st: NotificationType,
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

#[derive(Debug, Clone)]
pub struct SearchResponse<'a> {
    cache_control: usize,
    location: &'a str,
    server: &'a str,
    st: NotificationType,
    usn: USN,
    boot_id: usize,
    config_id: usize,
    search_port: u16,
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
USN: {usn}
BOOTID.UPNP.ORG: {boot_id}
CONFIGID.UPNP.ORG: {config_id}
SEARCHPORT.UPNP.ORG: {search_port}\r\n\r\n",
            cache_control = self.cache_control,
            location = self.location,
            server = self.server,
            st = self.st,
            usn = self.usn,
            boot_id = self.boot_id,
            config_id = self.config_id,
            search_port = self.search_port,
        )
    }
}

impl<'a> TryFrom<&'a str> for SearchResponse<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug, Clone)]
pub enum NotificationType {
    /// `ssdp:all` A wildcard value that indicates the search is for all devices and services on the network. This is used to discover any UPnP device or service
    All,
    /// `upnp:rootdevice` A root device is a device that can be used to discover other UPnP devices and services.
    RootDevice,
    /// The UUID represents a unique identifier for a device or service.
    Uuid(uuid::Uuid),
    Urn(urn::URN),
}

impl FromStr for NotificationType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ssdp:all" => Self::All,
            "upnp:rootdevice" => Self::RootDevice,
            rest if rest.starts_with("urn:") => Self::Urn(urn::URN::from_str(rest)?),
            rest if rest.starts_with("uuid:") => Self::Uuid(
                rest.strip_prefix("uuid:")
                    .expect("prefix checked above")
                    .parse()?,
            ),
            rest => Err(anyhow::anyhow!("Unknown notification type: {rest}"))?,
        })
    }
}

impl Display for NotificationType {
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

#[derive(Debug, Clone)]
pub struct NotifyByeByeMessage {
    pub host: SocketAddr,
    /// The Unique Service Name, which combines a unique identifier (UUID) with the device or service type.
    /// This allows clients to uniquely identify the device or service instance
    pub usn: USN,
    /// Notification type. Specifies type of device/service.
    pub nt: NotificationType,
    /// Notification subtype. Specifies type of notification.
    pub nts: NotificationSubType,
}

impl NotifyByeByeMessage {
    fn media_server() -> Self {
        NotifyByeByeMessage {
            host: SSDP_ADDR,
            usn: USN::device_uuid(UDN::new(SERVER_UUID)),
            nt: NotificationType::RootDevice,
            nts: NotificationSubType::ByeBye,
        }
    }
}

impl Display for NotifyByeByeMessage {
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

#[derive(Debug, Clone)]
pub struct NotifyUpdateMessage<'a> {
    pub host: SocketAddr,
    /// The Unique Service Name, which combines a unique identifier (UUID) with the device or service type.
    /// This allows clients to uniquely identify the device or service instance
    pub usn: &'a str,
    /// Url of device description
    pub location: &'a str,
    /// Notification type. Specifies type of device/service.
    pub nt: NotificationType,
    /// Notification subtype. Specifies type of notification.
    pub nts: NotificationSubType,
}

impl NotifyAliveMessage<'_> {}

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

#[derive(Debug, Clone)]
pub struct NotifyAliveMessage<'a> {
    pub host: SocketAddr,
    /// Url of device description
    pub location: Cow<'a, str>,
    /// The Unique Service Name, which combines a unique identifier (UUID) with the device or service type.
    /// This allows clients to uniquely identify the device or service instance
    pub usn: USN,
    /// Notification type. Specifies type of device/service.
    pub nt: NotificationType,
    /// Notification subtype. Specifies type of notification.
    pub nts: NotificationSubType,
    /// Cache life time in seconds
    pub cache_control: usize,
    /// Information about the software used by the origin server to handle the request
    pub server: &'a str,
}

impl NotifyAliveMessage<'_> {
    pub fn new(addr: IpAddr, nt: NotificationType, usn: USN) -> Self {
        let port: config::Port = config::CONFIG.get_value();
        let location = format!(
            "http://{addr}:{port}/upnp{path}",
            addr = addr,
            port = port.0,
            path = router::DESC_PATH
        );
        Self {
            host: SSDP_ADDR,
            location: Cow::Owned(location),
            usn,
            nt,
            nts: NotificationSubType::Alive,
            cache_control: CACHE_CONTROL,
            server: SERVER,
        }
    }

    pub fn first_server_message(addr: IpAddr) -> Self {
        let udn = UDN::new(SERVER_UUID);
        Self::new(addr, NotificationType::RootDevice, USN::root_device(udn))
    }

    pub fn second_server_message(addr: IpAddr) -> Self {
        let udn = UDN::new(SERVER_UUID);
        Self::new(
            addr,
            NotificationType::Uuid(SERVER_UUID),
            USN::device_uuid(udn),
        )
    }

    pub fn third_server_message(addr: IpAddr) -> Self {
        let udn = UDN::new(SERVER_UUID);
        let urn = urn::URN::media_server();
        Self::new(addr, NotificationType::Urn(urn.clone()), USN::urn(udn, urn))
    }

    pub fn service_message(addr: IpAddr, service_urn: urn::URN) -> Self {
        let udn = UDN::new(SERVER_UUID);
        let usn = USN::urn(udn, service_urn.clone());
        Self::new(addr, NotificationType::Urn(service_urn), usn)
    }
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
                        "st" => st = Some(NotificationType::from_str(value)?),
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
                        "nt" => nt = Some(NotificationType::from_str(value)?),
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
                            location: Cow::Borrowed(location),
                            usn: USN::from_str(usn)?,
                            nt,
                            nts,
                            cache_control,
                            server,
                        };
                        Ok(BroadcastMessage::NotifyAlive(notify_message))
                    }
                    NotificationSubType::ByeBye => {
                        let byebye_message = NotifyByeByeMessage {
                            host,
                            usn: USN::from_str(usn)?,
                            nt,
                            nts,
                        };
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
