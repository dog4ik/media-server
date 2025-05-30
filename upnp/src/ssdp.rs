use core::str;
use std::{
    borrow::Cow,
    fmt::Display,
    io::{Cursor, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    ops::Range,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use rand::Rng;
use socket2::{Domain, Protocol, Type};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::templates::UpnpAgent;

use super::{
    device_description::{self, Udn},
    router, urn,
};

pub(crate) const SSDP_IP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub(crate) const SSDP_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(SSDP_IP_ADDR, 1900));
const NOTIFY_INTERVAL_DURATION: Duration = Duration::from_secs(90);
pub const DEFAULT_SSDP_TTL: u32 = 2;

const CACHE_CONTROL: usize = 1800;

async fn sleep_rand_millis_duration(range: &Range<u64>) {
    let range = {
        let mut rng = rand::rng();
        rng.random_range(range.clone())
    };
    tokio::time::sleep(Duration::from_millis(range)).await;
}

fn bind_ssdp_socket(ttl: Option<u32>) -> anyhow::Result<UdpSocket> {
    let local_ip = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1900);
    let socket = socket2::Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_ttl(ttl.unwrap_or(DEFAULT_SSDP_TTL))?;
    socket.set_reuse_address(true)?;
    #[cfg(target_os = "linux")]
    socket.set_reuse_port(false)?;
    socket.set_nonblocking(true)?;
    socket.set_multicast_loop_v4(true)?;
    socket.join_multicast_v4(&SSDP_IP_ADDR, &Ipv4Addr::UNSPECIFIED)?;
    socket.bind(&SocketAddr::V4(local_ip).into())?;
    let socket = UdpSocket::from_std(socket.into())?;
    Ok(socket)
}

async fn resolve_local_addr() -> anyhow::Result<SocketAddr> {
    let google = Ipv4Addr::new(8, 8, 8, 8);
    // NOTE: this feels wrong. Find the better solution
    let socket =
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))).await?;
    socket
        .connect(SocketAddr::V4(SocketAddrV4::new(google, 0)))
        .await?;
    socket.local_addr().context("get local addr")
}

#[derive(Debug, Clone)]
pub struct SsdpListenerConfig {
    pub location_port: u16,
    pub ttl: Option<u32>,
    pub user_agent: UpnpAgent<'static>,
    pub uuid: uuid::Uuid,
}

#[derive(Debug)]
pub struct SsdpListener {
    socket: Arc<UdpSocket>,
    uuid: uuid::Uuid,
    boot_id: usize,
    location: String,
    config_id: usize,
    user_agent: UpnpAgent<'static>,
}

impl SsdpListener {
    pub async fn bind(config: SsdpListenerConfig) -> anyhow::Result<Self> {
        let socket = bind_ssdp_socket(config.ttl).context("failed to bind ssdp socket")?;
        // NOTE: maybe pass location via config?
        let local_addr = resolve_local_addr().await?;
        tracing::debug!("Resolved local ip address {local_addr}");
        let location = format!(
            "http://{addr}:{port}/upnp{path}",
            addr = local_addr.ip(),
            port = config.location_port,
            path = router::DESC_PATH
        );

        Ok(Self {
            socket: Arc::new(socket),
            boot_id: 8399389,
            location,
            config_id: 9999,
            user_agent: config.user_agent,
            uuid: config.uuid,
        })
    }

    pub async fn listen(&mut self, cancellation_token: CancellationToken) -> anyhow::Result<()> {
        let default_announce = Announce {
            cache_control: CACHE_CONTROL,
            location: self.location.clone(),
            server: self.user_agent.to_string(),
            notification_type: NotificationType::RootDevice,
            usn: USN::root_device(Udn::new(self.uuid)),
            boot_id: self.boot_id,
            config_id: self.config_id,
            search_port: None,
        };
        let mut announcer = Announcer::<MulticastAnnounce>::new(
            self.uuid,
            self.socket.clone(),
            SSDP_ADDR,
            default_announce,
        );
        announcer.announce_all().await?;

        let mut notify_interval = tokio::time::interval(NOTIFY_INTERVAL_DURATION);
        notify_interval.tick().await;

        let mut buf = [0; 2048];
        loop {
            tokio::select! {
                Ok((read, sender)) = self.socket.recv_from(&mut buf) => {
                    let data = &buf[..read];
                    if let Err(e) = self.handle_message(data, sender).await {
                        tracing::warn!("Failed to handle ssdp message: {e}");
                    };
                }
                _ = cancellation_token.cancelled() => {
                    self.handle_shutdown().await?;
                    return Ok(())
                }
                _ = notify_interval.tick() => {
                    announcer.announce_all().await?;
                }
            }
        }
    }

    async fn handle_message(&mut self, data: &[u8], sender: SocketAddr) -> anyhow::Result<()> {
        let payload = str::from_utf8(data).context("construct string from bytes")?;
        let message = BroadcastMessage::parse_ssdp_payload(payload)?;
        match message {
            BroadcastMessage::Search(msg) => {
                let default_announce = Announce {
                    cache_control: CACHE_CONTROL,
                    location: self.location.clone(),
                    server: self.user_agent.to_string(),
                    notification_type: NotificationType::RootDevice,
                    usn: USN::root_device(Udn::new(self.uuid)),
                    boot_id: self.boot_id,
                    config_id: self.config_id,
                    search_port: None,
                };
                let socket = self.socket.clone();
                let search_target = msg.st.clone();
                tracing::debug!(
                    user_agent = ?msg.user_agent,
                    mx = ?msg.mx,
                    st = %msg.st,
                    addr = %sender,
                    "Search message"
                );
                let mut announcer =
                    Announcer::<UnicastAnnounce>::new(self.uuid, socket, sender, default_announce);
                tokio::spawn(async move {
                    if let Some(mx) = msg.mx {
                        let sleep_range = 1..(mx.saturating_sub(1) as u64).clamp(1, 5) * 1000;
                        sleep_rand_millis_duration(&sleep_range).await;
                    }
                    match search_target {
                        NotificationType::All => {
                            announcer.announce_all().await?;
                        }
                        NotificationType::RootDevice => {
                            announcer.root_announce().await?;
                        }
                        NotificationType::Uuid(uuid) if uuid == announcer.server_uuid => {
                            announcer.root_announce().await?;
                        }
                        NotificationType::Uuid(_) => {}
                        NotificationType::Urn(ref urn) => match urn.urn_type {
                            urn::UrnType::Device(urn::DeviceType::MediaServer) => {
                                announcer.urn_announce(urn.clone()).await?;
                            }
                            urn::UrnType::Service(urn::ServiceType::ContentDirectory) => {
                                announcer.urn_announce(urn.clone()).await?;
                            }
                            urn::UrnType::Service(urn::ServiceType::ConnectionManager) => {
                                announcer.urn_announce(urn.clone()).await?;
                            }
                            _ => {}
                        },
                    };
                    Ok::<_, anyhow::Error>(())
                });
            }
            BroadcastMessage::NotifyAlive(alive) => {
                tracing::trace!(nt = %alive.nt, "Received alive message");
            }
            BroadcastMessage::NotifyByeBye(byebye) => {
                tracing::trace!(nt = %byebye.nt, "Received byebye message");
            }
            BroadcastMessage::NotifyUpdate(update) => {
                tracing::trace!(nt = %update.nt, "Received update message");
            }
        }
        Ok(())
    }

    async fn handle_shutdown(&self) -> anyhow::Result<()> {
        let self_byebye_message = NotifyByeByeMessage::media_server(self.boot_id, self.uuid);
        tracing::info!("Sending bye bye message");
        self.socket
            .send_to(self_byebye_message.to_string().as_bytes(), SSDP_ADDR)
            .await?;
        Ok(())
    }
}

#[derive(Debug)]
struct Announcer<T: AnnounceHandler> {
    p: std::marker::PhantomData<T>,
    server_uuid: uuid::Uuid,
    socket: Arc<UdpSocket>,
    sender: SocketAddr,
    default_announce: Announce,
    buf: Cursor<Vec<u8>>,
}

impl<T: AnnounceHandler> Announcer<T> {
    pub fn new(
        uuid: uuid::Uuid,
        socket: Arc<UdpSocket>,
        sender: SocketAddr,
        announce: Announce,
    ) -> Self {
        Self {
            p: std::marker::PhantomData,
            server_uuid: uuid,
            socket,
            sender,
            default_announce: announce,
            buf: Default::default(),
        }
    }

    pub async fn send_announce(&mut self) -> anyhow::Result<()> {
        T::handle_announce(&self.default_announce, &mut self.buf)?;
        let pos = self.buf.position() as usize;
        self.socket
            .send_to(&self.buf.get_ref()[..pos], self.sender)
            .await?;
        self.buf.set_position(0);
        Ok(())
    }

    pub async fn urn_announce(&mut self, urn: urn::URN) -> anyhow::Result<()> {
        let udn = Udn::new(self.server_uuid);
        self.default_announce.notification_type = NotificationType::Urn(urn.clone());
        self.default_announce.usn = USN::urn(udn, urn);
        self.send_announce().await?;
        tracing::debug!("Finished announcing urn to {}", self.sender);
        Ok(())
    }

    pub async fn root_announce(&mut self) -> anyhow::Result<()> {
        self.default_announce.notification_type = NotificationType::RootDevice;
        self.default_announce.usn = USN::root_device(Udn::new(self.server_uuid));
        self.send_announce().await?;
        Ok(())
    }

    pub async fn announce_all(&mut self) -> anyhow::Result<()> {
        let udn = Udn::new(self.server_uuid);
        self.default_announce.notification_type = NotificationType::RootDevice;
        self.default_announce.usn = USN::root_device(udn.clone());
        self.send_announce().await?;

        self.default_announce.notification_type = NotificationType::Uuid(self.server_uuid);
        self.default_announce.usn = USN::device_uuid(udn.clone());
        self.send_announce().await?;

        let urn = urn::URN::media_server();
        self.default_announce.notification_type = NotificationType::Urn(urn.clone());
        self.default_announce.usn = USN::urn(udn.clone(), urn);
        self.send_announce().await?;

        let urn = urn::URN {
            version: 1,
            urn_type: urn::UrnType::Service(urn::ServiceType::ContentDirectory),
        };
        self.default_announce.notification_type = NotificationType::Urn(urn.clone());
        self.default_announce.usn = USN::urn(udn.clone(), urn);
        self.send_announce().await?;

        let urn = urn::URN {
            version: 1,
            urn_type: urn::UrnType::Service(urn::ServiceType::ConnectionManager),
        };
        self.default_announce.notification_type = NotificationType::Urn(urn.clone());
        self.default_announce.usn = USN::urn(udn, urn);
        self.send_announce().await?;
        tracing::debug!("Finished announcing everything to: {}", self.sender);
        Ok(())
    }
}

///  Unique Service Name. Identifies a unique instance of a device or service.
#[derive(Debug, Clone)]
pub struct USN {
    udn: device_description::Udn,
    kind: USNkind,
}

#[derive(Debug, Clone)]
pub enum USNkind {
    RootDevice,
    DeviceUuid,
    URN(urn::URN),
}

impl USN {
    pub const fn root_device(udn: device_description::Udn) -> Self {
        Self {
            udn,
            kind: USNkind::RootDevice,
        }
    }
    pub const fn device_uuid(udn: device_description::Udn) -> Self {
        Self {
            udn,
            kind: USNkind::DeviceUuid,
        }
    }
    pub const fn urn(udn: device_description::Udn, urn: urn::URN) -> Self {
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
            let udn = device_description::Udn::from_str(s)?;
            return Ok(Self::device_uuid(udn));
        };
        let udn = device_description::Udn::from_str(start)?;

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
    pub st: NotificationType,
    /// Field value contains maximum wait time in seconds. shall be greater than or equal to 1 and should
    /// be less than 5 inclusive. Device responses should be delayed a random duration between 0 and this many
    /// seconds to balance load for the control point when it processes responses. This value is allowed to be
    /// increased if a large number of devices are expected to respond
    /// Missing in unicast search message
    pub mx: Option<usize>,
    /// Same as server in search messages
    pub user_agent: Option<&'a str>,
    /// A control point can request that a device replies to a TCP port on the control point
    /// Missing in unicast search message
    pub tcp_port: Option<u16>,
    /// Specifies the friendly name of the control point. The friendly name is vendor specific.
    /// Missing in unicast search message
    pub cp_fn: Option<&'a str>,
    /// Uuid of the control point.
    /// Missing in unicast search message
    /// Optional in multicast search message
    pub cp_uuid: Option<&'a str>,
}

impl SearchMessage<'_> {
    pub fn is_unicast(&self) -> bool {
        self.mx.is_none()
    }
}

impl Display for SearchMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "M-SEARCH * HTTP/1.1\r\n\
HOST: {host}\r\n\
MAN: \"ssdp:discover\"\r\n\
ST: {search_target}\r\n",
            host = self.host,
            search_target = self.st,
        )?;
        if let Some(user_agent) = self.user_agent {
            write!(f, "USER-AGENT: {user_agent}\r\n")?;
        }
        if let Some(mx) = self.mx {
            write!(f, "MX: {mx}\r\n")?;
        }
        if let Some(tcp_port) = self.tcp_port {
            write!(f, "TCPPORT.UPNP.ORG: {tcp_port}\r\n")?;
        }
        if let Some(cp_fn) = self.cp_fn {
            write!(f, "CPFN.UPNP.ORG: {cp_fn}\r\n")?;
        }
        write!(f, "\r\n")
    }
}

pub trait AnnounceHandler {
    fn handle_announce(announce: &Announce, f: impl Write) -> anyhow::Result<()>;
    fn parse_announce(announce: &str) -> anyhow::Result<Announce>;
}

/// Multicast announce aka notify with nts: ssdp:alive or advertisement
pub struct MulticastAnnounce;

impl AnnounceHandler for MulticastAnnounce {
    fn handle_announce(a: &Announce, mut f: impl Write) -> anyhow::Result<()> {
        write!(
            f,
            "NOTIFY * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
CACHE-CONTROL: max-age={cache_control}\r\n\
LOCATION: {location}\r\n\
NT: {nt}\r\n\
NTS: ssdp:alive\r\n\
SERVER: {server}\r\n\
USN: {usn}\r\n\
BOOTID.UPNP.ORG: {boot_id}\r\n\
CONFIGID.UPNP.ORG: {config_id}\r\n",
            cache_control = a.cache_control,
            location = a.location,
            nt = a.notification_type,
            server = a.server,
            usn = a.usn,
            boot_id = a.boot_id,
            config_id = a.config_id,
        )?;
        if let Some(search_port) = a.search_port {
            write!(f, "SEARCHPORT.UPNP.ORG: {search_port}\r\n")?;
        }
        write!(f, "\r\n")?;
        Ok(())
    }

    fn parse_announce(announce: &str) -> anyhow::Result<Announce> {
        let mut cache_control = None;
        let mut location = None;
        let mut server = None;
        let mut notification_type = None;
        let mut usn = None;
        let mut boot_id = None;
        let mut config_id = None;
        let search_port = None;
        let mut lines = announce.lines();
        anyhow::ensure!(lines.next() == Some("NOTIFY * HTTP/1.1"));
        let headers = lines.filter_map(|l| l.split_once(':'));

        for (name, value) in headers {
            let value = value.trim();
            match name.to_ascii_lowercase().as_str() {
                "cache-control" => {
                    let (prefix, cache_duration) =
                        value.split_once('=').context("split cache control")?;
                    anyhow::ensure!(prefix.trim() == "max-age");
                    cache_control = Some(cache_duration.parse().context("parse duration seconds")?)
                }
                "location" => location = Some(value.to_owned()),
                "server" => server = Some(value.to_owned()),
                "nt" => notification_type = NotificationType::from_str(value).map(Some)?,
                "nts" => anyhow::ensure!(value == "ssdp:alive"),
                "usn" => usn = USN::from_str(value).map(Some)?,
                "configid.upnp.org" => config_id = Some(value.parse().context("parse configid")?),
                "bootid.upnp.org" => boot_id = Some(value.parse().context("parse boot_id")?),
                _ => (),
            }
        }

        let cache_control = cache_control.context("parse cache_control")?;
        let location = location.context("parse location")?;
        let server = server.context("parse server")?;
        let notification_type = notification_type.context("parse notification_type")?;
        let usn = usn.context("parse usn")?;
        let boot_id = boot_id.unwrap_or_default();
        let config_id = config_id.unwrap_or_default();

        Ok(Announce {
            cache_control,
            location,
            server,
            notification_type,
            usn,
            boot_id,
            config_id,
            search_port,
        })
    }
}
/// Uniscast announce aka search response.
pub struct UnicastAnnounce;

impl AnnounceHandler for UnicastAnnounce {
    fn handle_announce(a: &Announce, mut f: impl Write) -> anyhow::Result<()> {
        let now = time::OffsetDateTime::now_utc();
        let format = time::format_description::parse_borrowed::<2>("[weekday repr:short], [day padding:zero] [month repr:short] [year] [hour]:[minute]:[second] GMT").expect("infallible");
        let formatted_date = now.format(&format).expect("infallible");
        write!(
            f,
            "HTTP/1.1 200 OK\r\n\
CACHE-CONTROL: max-age={cache_control}\r\n\
LOCATION: {location}\r\n\
SERVER: {server}\r\n\
EXT:\r\n\
ST: {st}\r\n\
DATE: {date}\r\n\
USN: {usn}\r\n\
BOOTID.UPNP.ORG: {boot_id}\r\n\
CONFIGID.UPNP.ORG: {config_id}\r\n",
            cache_control = a.cache_control,
            location = a.location,
            server = a.server,
            st = a.notification_type,
            date = formatted_date,
            usn = a.usn,
            boot_id = a.boot_id,
            config_id = a.config_id,
        )?;
        if let Some(search_port) = a.search_port {
            write!(f, "SEARCHPORT.UPNP.ORG: {search_port}\r\n")?;
        }
        write!(f, "\r\n")?;
        Ok(())
    }

    fn parse_announce(announce: &str) -> anyhow::Result<Announce> {
        let mut cache_control = None;
        let mut location = None;
        let mut server = None;
        let mut notification_type = None;
        let mut usn = None;
        let mut boot_id = None;
        let mut config_id = None;
        let mut search_port = None;
        let mut lines = announce.lines();
        anyhow::ensure!(lines.next() == Some("HTTP/1.1 200 OK"));
        let headers = lines.filter_map(|l| l.split_once(':'));

        for (name, value) in headers {
            let value = value.trim();
            match name.to_ascii_lowercase().as_str() {
                "cache-control" => {
                    let (prefix, cache_duration) =
                        value.split_once('=').context("split cache control")?;
                    anyhow::ensure!(prefix.trim() == "max-age");
                    cache_control = Some(cache_duration.parse().context("parse duration seconds")?)
                }
                "location" => location = Some(value.to_owned()),
                "server" => server = Some(value.to_owned()),
                "st" => notification_type = NotificationType::from_str(value).map(Some)?,
                "usn" => usn = USN::from_str(value).map(Some)?,
                "searchport.upnp.org" => {
                    search_port = Some(value.parse().context("parse searchport")?)
                }
                "configid.upnp.org" => config_id = Some(value.parse().context("parse configid")?),
                "bootid.upnp.org" => boot_id = Some(value.parse().context("parse boot_id")?),
                _ => (),
            }
        }

        let cache_control = cache_control.context("parse cache_control")?;
        let location = location.context("parse location")?;
        let server = server.context("parse server")?;
        let notification_type = notification_type.context("parse notification_type")?;
        let usn = usn.context("parse usn")?;
        let boot_id = boot_id.unwrap_or_default();
        let config_id = config_id.unwrap_or_default();

        Ok(Announce {
            cache_control,
            location,
            server,
            notification_type,
            usn,
            boot_id,
            config_id,
            search_port,
        })
    }
}

/// Responses to search requests are intentionally parallel to advertisements, and as such,
/// follow the same pattern as listed for NOTIFY with ssdp:alive (above) except that instead of the NT
/// header field in response is an ST header field in advertisement (notification_type).
#[derive(Debug, Clone)]
pub struct Announce {
    pub cache_control: usize,
    pub location: String,
    pub server: String,
    pub notification_type: NotificationType,
    pub usn: USN,
    pub boot_id: usize,
    pub config_id: usize,
    pub search_port: Option<usize>,
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
    pub boot_id: usize,
    pub config_id: usize,
}

impl NotifyByeByeMessage {
    fn media_server(boot_id: usize, uuid: uuid::Uuid) -> Self {
        NotifyByeByeMessage {
            host: SSDP_ADDR,
            usn: USN::device_uuid(Udn::new(uuid)),
            nt: NotificationType::RootDevice,
            nts: NotificationSubType::ByeBye,
            boot_id,
            config_id: 0,
        }
    }
}

impl Display for NotifyByeByeMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NOTIFY * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
NT: {nt}\r\n\
NTS: {nts}\r\n\
USN: {usn}\r\n\
BOOTID.UPNP.ORG: {boot_id}\r\n\
CONFIGID.UPNP.ORG: {config_id}\r\n\r\n",
            nt = self.nt,
            nts = self.nts,
            usn = self.usn,
            boot_id = self.boot_id,
            config_id = self.config_id,
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
    pub boot_id: usize,
    pub config_id: usize,
    pub next_boot_id: usize,
    pub search_port: Option<u16>,
}

impl Display for NotifyUpdateMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NOTIFY * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
LOCATION: {location}\r\n\
NT: {nt}\r\n\
NTS: {nts}\r\n\
USN: {usn}\r\n\
BOOTID.UPNP.ORG: {boot_id}\r\n\
CONFIGID.UPNP.ORG: {config_id}\r\n\
NEXTBOOTID.UPNP.ORG: {next_boot_id}\r\n",
            location = self.location,
            nt = self.nt,
            nts = self.nts,
            usn = self.usn,
            boot_id = self.boot_id,
            config_id = self.config_id,
            next_boot_id = self.next_boot_id,
        )?;
        if let Some(search_port) = self.search_port {
            write!(f, "SEARCHPORT.UPNP.ORG: {search_port}\r\n")?;
        }
        write!(f, "\r\n")
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
    pub boot_id: usize,
    pub config_id: usize,
    pub search_port: Option<u16>,
}

impl Display for NotifyAliveMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NOTIFY * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
CACHE-CONTROL: max-age={cache_control}\r\n\
LOCATION: {location}\r\n\
NT: {nt}\r\n\
NTS: {nts}\r\n\
SERVER: {server}\r\n\
USN: {usn}\r\n\
BOOTID.UPNP.ORG: {boot_id}\r\n\
CONFIGID.UPNP.ORG: {config_id}\r\n",
            cache_control = self.cache_control,
            location = self.location,
            nt = self.nt,
            nts = self.nts,
            server = self.server,
            usn = self.usn,
            boot_id = self.boot_id,
            config_id = self.config_id,
        )?;
        if let Some(search_port) = self.search_port {
            write!(f, "SEARCHPORT.UPNP.ORG: {search_port}\r\n")?;
        }
        write!(f, "\r\n")
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
                let mut st = None;
                let mut mx = None;
                let mut user_agent = None;
                let mut cp_fn = None;
                let mut cp_uuid = None;
                let mut tcp_port = None;
                for (name, value) in headers {
                    let value = value.trim();
                    match name.to_ascii_lowercase().as_str() {
                        "host" => {
                            host = Some(SocketAddr::V4(
                                SocketAddrV4::from_str(value).context("parse host address")?,
                            ));
                        }
                        "st" => st = Some(NotificationType::from_str(value)?),
                        "mx" => mx = Some(value.parse()?),
                        "user-agent" => user_agent = Some(value),
                        "cpfn.upnp.org" => cp_fn = Some(value),
                        "cpuuid.upnp.org" => cp_uuid = Some(value),
                        "tcpport.upnp.org" => {
                            tcp_port = Some(value.parse().context("parse tcp port")?)
                        }
                        _ => (),
                    }
                }
                let host = host.context("missing host")?;
                let st = st.context("missing st")?;
                // Compatibility with upnp 1.0
                let search_message = SearchMessage {
                    host,
                    st,
                    mx,
                    user_agent,
                    cp_fn,
                    cp_uuid,
                    tcp_port,
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
                let mut boot_id = None;
                let mut config_id = None;
                let mut search_port = None;
                let mut next_boot_id = None;
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
                        "bootid.upnp.org" => {
                            boot_id = Some(value.parse().context("parse boot id")?)
                        }
                        "configid.upnp.org" => {
                            config_id = Some(value.parse().context("parse config id")?)
                        }
                        "searchport.upnp.org" => {
                            search_port = Some(value.parse().context("parse search port")?)
                        }
                        "nextbootid.upnp.org" => {
                            next_boot_id = Some(value.parse().context("parse next boot id")?)
                        }
                        _ => (),
                    }
                }
                let nt = nt.context("missing nt")?;
                let nts = nts.context("missing nts")?;
                let host = host.context("missing host")?;
                let usn = usn.context("missing usn")?;
                let boot_id = boot_id.unwrap_or_default();
                let config_id = config_id.unwrap_or_default();
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
                            boot_id,
                            config_id,
                            search_port,
                        };
                        Ok(BroadcastMessage::NotifyAlive(notify_message))
                    }
                    NotificationSubType::ByeBye => {
                        let byebye_message = NotifyByeByeMessage {
                            host,
                            usn: USN::from_str(usn)?,
                            nt,
                            nts,
                            boot_id,
                            config_id,
                        };
                        Ok(BroadcastMessage::NotifyByeBye(byebye_message))
                    }
                    NotificationSubType::Update => {
                        let location = location.context("missing location")?;
                        let next_boot_id = next_boot_id.context("missing next boot id")?;
                        let update_message = NotifyUpdateMessage {
                            location,
                            host,
                            usn,
                            nt,
                            nts,
                            boot_id,
                            config_id,
                            next_boot_id,
                            search_port,
                        };
                        Ok(BroadcastMessage::NotifyUpdate(update_message))
                    }
                }
            }
            _ => Err(anyhow::anyhow!("Unknown method encountered: {method}")),
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
