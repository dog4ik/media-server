use std::net::Ipv4Addr;

use port_listing::{ArgPortListing, PortMappingEntry};
use quick_xml::events::{BytesText, Event};

use crate::{
    IntoXml,
    service_client::{ActionCallError, ScpdClient, ScpdService},
    service_variables::{IntoUpnpValue, SVariable},
    urn::{ServiceType, URN, UrnType},
};

/// Information on the connection types used in the gateway
#[derive(Default, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConnectionType {
    /// Valid connection types cannot be identified. This MAY be due to the fact that the LinkType variable (if
    /// specified in the WAN*LinkConfig service) is uninitialized.
    Unconfigured,
    /// The Internet Gateway is an IP router between the LAN and the WAN connection.
    #[default]
    IpRouted,
    /// The Internet Gateway is an Ethernet bridge between the LAN and the WAN connection. A router at the
    /// other end of the WAN connection from the IGD routes IP packets
    IpBridged,
}

impl IntoUpnpValue for ConnectionType {
    const TYPE_NAME: crate::service_variables::DataType =
        crate::service_variables::DataType::String;

    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match value {
            "Unconfigured" => Ok(Self::Unconfigured),
            "IP_Routed" => Ok(Self::IpRouted),
            "IP_Bridged" => Ok(Self::IpBridged),
            _ => Err(anyhow::anyhow!("unknown ConnectionType value: {value}")),
        }
    }
}

impl IntoXml for ConnectionType {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let val = match self {
            Self::Unconfigured => "Unconfigured",
            Self::IpRouted => "IP_Routed",
            Self::IpBridged => "IP_Bridged",
        };
        w.write_event(Event::Text(BytesText::new(val)))
    }
}

impl SVariable for ConnectionType {
    type VarType = Self;
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&["Unconfigured", "IP_Routed", "IP_Bridged"]);
    const VAR_NAME: &str = "ConnectionType";
}

/// This variable represents a CSV list indicating the types of connections possible in the context of a specific
/// modem and link type. Possible values are a subset or proper subset of values listed in Table 2-3 in the specification.
#[derive(Debug)]
pub struct PossibleConnectionTypes;

impl SVariable for PossibleConnectionTypes {
    type VarType = String;

    const VAR_NAME: &str = "PossibleConnectionTypes";
}

/// Contains information on the status of the connection
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ConnectionStatus {
    /// This value indicates that other variables in the service table are uninitialized or in an invalid state. Examples
    /// of such variables include PossibleConnectionTypes and ConnectionType
    Unconfigured,
    /// The WANConnectionDevice is in the process of initiating a connection for the first time after the connection
    /// became disconnected
    Connecting,
    /// At least one client has successfully initiated an Internet connection using this instance.
    Connected,
    /// The connection is active (packets are allowed to flow through), but will transition to Disconnecting state
    /// after a certain period (indicated by WarnDisconnectDelay).
    PendingDisconnect,
    /// The WANConnectionDevice is in the process of terminating a connection. On successful termination,
    /// ConnectionStatus transitions to Disconnected.
    Disconnecting,
    /// No ISP connection is active (or being activated) from this connection instance. No packets are transiting the
    /// gateway.
    Disconnected,
}
impl IntoUpnpValue for ConnectionStatus {
    const TYPE_NAME: crate::service_variables::DataType =
        crate::service_variables::DataType::String;

    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match value {
            "Unconfigured" => Ok(Self::Unconfigured),
            "Connecting" => Ok(Self::Connecting),
            "Connected" => Ok(Self::Connected),
            "PendingDisconnect" => Ok(Self::PendingDisconnect),
            "Disconnecting" => Ok(Self::Disconnecting),
            "Disconnected" => Ok(Self::Disconnected),
            _ => Err(anyhow::anyhow!("unknown ConnectionStatus value: {value}")),
        }
    }
}

impl IntoXml for ConnectionStatus {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let val = match self {
            Self::Unconfigured => "Unconfigured",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::PendingDisconnect => "PendingDisconnect",
            Self::Disconnecting => "Disconnecting",
            Self::Disconnected => "Disconnected",
        };
        w.write_event(Event::Text(BytesText::new(val)))
    }
}

impl SVariable for ConnectionStatus {
    type VarType = Self;
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "Unconfigured",
        "Connecting",
        "Connected",
        "PendingDisconnect",
        "Disconnecting",
        "Disconnected",
    ]);
    const VAR_NAME: &str = "ConnectionStatus";
}

/// The variable Uptime represents time in seconds that this connections has stayed up.
#[derive(Debug)]
pub struct UpTime;

impl SVariable for UpTime {
    type VarType = u32;
    const VAR_NAME: &str = "UpTime";
}

/// This variable is a string that provides information about the cause of failure for the last connection setup
/// attempt
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LastConnectionError {
    None,
    CommandAborted,
    NotEnabledForInternet,
    IspDisconnect,
    UserDisconnect,
    IdleDisconnect,
    ForcedDisconnect,
    NoCarrier,
    IpConfiguration,
    Unknown,
}

impl IntoUpnpValue for LastConnectionError {
    const TYPE_NAME: crate::service_variables::DataType =
        crate::service_variables::DataType::String;

    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match value {
            "ERROR_NONE" => Ok(Self::None),
            "ERROR_COMMAND_ABORTED" => Ok(Self::CommandAborted),
            "ERROR_NOT_ENABLED_FOR_INTERNET" => Ok(Self::NotEnabledForInternet),
            "ERROR_ISP_DISCONNECT" => Ok(Self::IspDisconnect),
            "ERROR_USER_DISCONNECT" => Ok(Self::UserDisconnect),
            "ERROR_IDLE_DISCONNECT" => Ok(Self::IdleDisconnect),
            "ERROR_FORCED_DISCONNECT" => Ok(Self::ForcedDisconnect),
            "ERROR_NO_CARRIER" => Ok(Self::NoCarrier),
            "ERROR_IP_CONFIGURATION" => Ok(Self::IpConfiguration),
            "ERROR_UNKNOWN" => Ok(Self::Unknown),
            _ => Err(anyhow::anyhow!(
                "unknown LastConnectionError value: {value}"
            )),
        }
    }
}

impl IntoXml for LastConnectionError {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let val = match self {
            Self::None => "ERROR_NONE",
            Self::CommandAborted => "ERROR_COMMAND_ABORTED",
            Self::NotEnabledForInternet => "ERROR_NOT_ENABLED_FOR_INTERNET",
            Self::IspDisconnect => "ERROR_ISP_DISCONNECT",
            Self::UserDisconnect => "ERROR_USER_DISCONNECT",
            Self::IdleDisconnect => "ERROR_IDLE_DISCONNECT",
            Self::ForcedDisconnect => "ERROR_FORCED_DISCONNECT",
            Self::NoCarrier => "ERROR_NO_CARRIER",
            Self::IpConfiguration => "ERROR_IP_CONFIGURATION",
            Self::Unknown => "ERROR_UNKNOWN",
        };
        w.write_event(Event::Text(BytesText::new(val)))
    }
}

impl SVariable for LastConnectionError {
    type VarType = Self;
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "ERROR_NONE",
        "ERROR_COMMAND_ABORTED",
        "ERROR_NOT_ENABLED_FOR_INTERNET",
        "ERROR_ISP_DISCONNECT",
        "ERROR_USER_DISCONNECT",
        "ERROR_IDLE_DISCONNECT",
        "ERROR_FORCED_DISCONNECT",
        "ERROR_NO_CARRIER",
        "ERROR_IP_CONFIGURATION",
        "ERROR_UNKNOWN",
    ]);
    const VAR_NAME: &str = "LastConnectionError";
}

/// The AutoDisconnectTime variable represents time in seconds (since the establishment of the connection –
/// measured from the time ConnectionStatus transitions to Connected), after which connection termination is
/// automatically initiated by the gateway.
///
/// This occurs irrespective of whether the connection is being used or
/// not. A value of zero for AutoDisconnectTime indicates that the connection is not to be turned off
/// automatically. However, this MAY be overridden by:
/// - An implementation specific WAN/Gateway device policy,
/// - EnabledForInternet variable (see WANCommonInterfaceConfig* ) being set to “0” (false) by a control point,
/// - Connection termination initiated by ISP.
/// If WarnDisconnectDelay is non-zero, the connection state is changed to PendingDisconnect. It stays in this
/// state for WarnDisconnectDelay seconds (if no connection requests are made) before switching to
/// Disconnected. The data type of this variable is ui4.
#[derive(Debug)]
pub struct AutoDisconnectTime;
impl SVariable for AutoDisconnectTime {
    type VarType = u32;

    const VAR_NAME: &str = "AutoDisconnectTime";
}

/// IdleDisconnectTime represents the idle time of a connection in seconds (since the establishment of the
/// connection), after which connection termination is initiated by the gateway.
///
/// A value of zero for this variable allows infinite idle time – connection will not be terminated due to idle time.
///
/// NOTE: Layer 2 heartbeat packets are included as part of an idle state i.e., they do not reset the idle timer.
#[derive(Debug)]
pub struct IdleDisconnectTime;
impl SVariable for IdleDisconnectTime {
    type VarType = u32;

    const VAR_NAME: &str = "IdleDisconnectTime";
}

/// This variable represents time in seconds the [ConnectionStatus] remains in the [ConnectionStatus::PendingDisconnect] state
/// before transitioning to [ConnectionStatus::Disconnecting] state to drop the connection.
///
/// For example, if this variable was set to 5 seconds,
/// and one of the clients terminates an active connection, the gateway will wait
/// (with [ConnectionStatus::PendingDisconnect]) for 5 seconds before actual termination of the connection. A value
/// of zero for this variable indicates that no warning will be given to clients before terminating the connection.
#[derive(Debug)]
pub struct WarnDisconnectDelay;
impl SVariable for WarnDisconnectDelay {
    type VarType = u32;

    const VAR_NAME: &str = "WarnDisconnectDelay";
}

/// This variable indicates if `Realm-specific IP` (RSIP) is available as a feature on the `Internet Gateway Device`.
#[derive(Debug)]
pub struct RSIPAvailable;
impl SVariable for RSIPAvailable {
    type VarType = bool;

    const VAR_NAME: &str = "RSIPAvailable";
}

/// This boolean type variable indicates if `Network Address Translation` (NAT) is enabled for this connection.
#[derive(Debug)]
pub struct NATEnabled;
impl SVariable for NATEnabled {
    type VarType = bool;

    const VAR_NAME: &str = "NATEnabled";
}

/// `ExternalIPAddress` is the external IP address used by NAT for the connection
#[derive(Debug)]
pub struct ExternalIPAddress;
impl SVariable for ExternalIPAddress {
    type VarType = Ipv4Addr;

    const VAR_NAME: &str = "ExternalIPAddress";
}

/// This variable indicates the number of NAT port mapping entries (number of elements in the array)
/// configured on this connection
#[derive(Debug)]
pub struct PortMappingNumberOfEntries;
impl SVariable for PortMappingNumberOfEntries {
    type VarType = u32;

    const VAR_NAME: &str = "PortMappingNumberOfEntries";
}

/// This variable allows security conscious users to disable and enable dynamic NAT port mappings on the IGD.
#[derive(Debug)]
pub struct PortMappingEnabled;
impl SVariable for PortMappingEnabled {
    type VarType = bool;

    const VAR_NAME: &str = "PortMappingEnabled";
}

/// This variable determines the lifetime in seconds of a port-mapping lease. Non-zero values indicate the
/// duration after which a port mapping will be removed, unless a control point refreshes the mapping
#[derive(Debug)]
pub struct PortMappingLeaseDuration;
impl PortMappingLeaseDuration {
    pub const MAX: std::time::Duration = std::time::Duration::from_secs(604800);
}
impl SVariable for PortMappingLeaseDuration {
    type VarType = u32;
    const RANGE: Option<crate::service_variables::Range> = Some(crate::service_variables::Range {
        start: 0,
        end: 604800,
        step: None,
    });

    const VAR_NAME: &str = "PortMappingLeaseDuration";
}

/// This variable represents the source of inbound IP packets.
#[derive(Debug)]
pub struct RemoteHost;

impl SVariable for RemoteHost {
    type VarType = Option<Ipv4Addr>;

    const VAR_NAME: &str = "RemoteHost";
}

/// This variable represents the external port that the `NAT` gateway would "listen" on for
/// connection requests to a corresponding [InternalPort] on an [InternalClient]
#[derive(Debug)]
pub struct ExternalPort;
impl SVariable for ExternalPort {
    type VarType = u16;

    const VAR_NAME: &str = "ExternalPort";
}

/// This variable is of type ui2 and represents the port on [InternalClient] that the gateway SHOULD forward
/// connection requests to. A value of 0 is not allowed.
#[derive(Debug)]
pub struct InternalPort;
impl SVariable for InternalPort {
    type VarType = u16;

    const VAR_NAME: &str = "InternalPort";
}

/// This variable represents the protocol of the port mapping
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PortMappingProtocol {
    TCP,
    UDP,
}

impl IntoUpnpValue for PortMappingProtocol {
    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match value {
            "TCP" => Ok(Self::TCP),
            "UDP" => Ok(Self::UDP),
            _ => Err(anyhow::anyhow!(
                "unknown PortMappingProtocol value: {value}"
            )),
        }
    }
}

impl IntoXml for PortMappingProtocol {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let val = match self {
            Self::TCP => "TCP",
            Self::UDP => "UDP",
        };
        w.write_event(Event::Text(BytesText::new(val)))
    }
}
impl SVariable for PortMappingProtocol {
    type VarType = Self;
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&["TCP", "UDP"]);

    const VAR_NAME: &str = "PortMappingProtocol";
}

/// This variable is a string containing the IP address or DNS host name of an `InternalClient`
#[derive(Debug)]
pub struct InternalClient;
impl SVariable for InternalClient {
    type VarType = Ipv4Addr;

    const VAR_NAME: &str = "InternalClient";
}

/// This is a string representation of a port mapping.
///
/// The format of the description string is not specified and is
/// application dependent. If specified, the description string can be displayed to a user via the UI of a control
/// point, enabling easier management of port mappings. The description string for a port mapping (or a set of
/// related port mappings) is NOT REQUIRED to be unique across multiple instantiations of an application on
/// multiple nodes in the residential LAN.
#[derive(Debug)]
pub struct PortMappingDescription;
impl SVariable for PortMappingDescription {
    type VarType = String;

    const VAR_NAME: &str = "PortMappingDescription";
}

/// The type of this variable is ui4, and it is used to notify of changes done in NAT or firewall rules.
///
/// # Examples:
/// - the user changed the firewall level settings of his IGD, and the NAT port mappings rules are no more valid,
/// - the user disabled the UPnP IGD NAT traversal facilities through the WWW-administration of the IGD,
/// - the user updated a NAT rule thanks to the WWW-administration of his IGD, and that NAT rule was previously created by a UPnP IGD control point
///
///
/// Whenever a change is done, the value of this variable is incremented by 1 and evented. A change can be an
/// addition, a removal, an update, or the fact that a rule is disabled or enabled. So, control points are
/// encouraged to check if their port mappings are still valid when notified.
///
/// This variable is evented when something which affects the port mappings validity occurs. Even if the event
/// affects several port mappings rules, the variable is evented once (and not for each impacted port mappings
/// rules).
#[derive(Debug)]
pub struct SystemUpdateID;
impl SVariable for SystemUpdateID {
    type VarType = u32;

    const VAR_NAME: &str = "SystemUpdateID";
}

/// This argument type is used to describe management intent when issuing certain actions with elevated level of access
#[derive(Debug)]
pub struct ArgManage;
impl SVariable for ArgManage {
    type VarType = bool;

    const VAR_NAME: &str = "A_ARG_TYPE_Manage";
}

pub mod port_listing {

    use anyhow::Context;
    use quick_xml::events::{BytesStart, Event};

    use crate::{
        FromXml, IntoXml, XmlReaderExt,
        service_variables::{IntoUpnpValue, SVariable},
    };

    use super::{
        ExternalPort, InternalClient, InternalPort, PortMappingDescription, PortMappingEnabled,
        PortMappingLeaseDuration, PortMappingProtocol, RemoteHost,
    };

    #[derive(Debug, Clone)]
    pub struct PortMappingEntry {
        pub new_remote_host: <RemoteHost as SVariable>::VarType,
        pub new_external_port: <ExternalPort as SVariable>::VarType,
        pub new_protocol: <PortMappingProtocol as SVariable>::VarType,
        pub new_internal_port: <InternalPort as SVariable>::VarType,
        pub new_internal_client: <InternalClient as SVariable>::VarType,
        pub new_enabled: <PortMappingEnabled as SVariable>::VarType,
        pub new_description: <PortMappingDescription as SVariable>::VarType,
        pub new_lease_time: <PortMappingLeaseDuration as SVariable>::VarType,
    }

    impl IntoXml for PortMappingEntry {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            let parent = BytesStart::new("p:PortMappingEntry");
            w.write_event(Event::Start(parent.clone()))?;
            w.create_element("p:NewRemoteHost")
                .write_inner_content(|w| self.new_remote_host.write_xml(w))?;
            w.create_element("p:NewExternalPort")
                .write_inner_content(|w| self.new_external_port.write_xml(w))?;
            w.create_element("p:NewProtocol")
                .write_inner_content(|w| self.new_protocol.write_xml(w))?;
            w.create_element("p:NewInternalPort")
                .write_inner_content(|w| self.new_internal_port.write_xml(w))?;
            w.create_element("p:NewInternalClient")
                .write_inner_content(|w| self.new_internal_client.write_xml(w))?;
            w.create_element("p:NewEnabled")
                .write_inner_content(|w| self.new_enabled.write_xml(w))?;
            w.create_element("p:NewDescription")
                .write_inner_content(|w| self.new_description.write_xml(w))?;
            w.create_element("p:NewLeaseTime")
                .write_inner_content(|w| self.new_lease_time.write_xml(w))?;
            w.write_event(Event::End(parent.to_end()))
        }
    }

    impl<'a> FromXml<'a> for PortMappingEntry {
        fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self>
        where
            Self: Sized,
        {
            let parent = r.read_to_start()?;
            anyhow::ensure!(parent.local_name().as_ref() == b"PortMappingEntry");
            let parent_end = parent.to_end().into_owned();
            let mut new_remote_host = None;
            let mut new_external_port = None;
            let mut new_protocol = None;
            let mut new_internal_port = None;
            let mut new_internal_client = None;
            let mut new_enabled = None;
            let mut new_description = None;
            let mut new_lease_time = None;

            loop {
                let tag_start = match r.read_event()? {
                    Event::Start(start) => start,
                    Event::End(end) => {
                        if end == parent_end {
                            break;
                        } else {
                            return Err(anyhow::anyhow!("expected parent end, got {:?}", end));
                        }
                    }
                    rest => return Err(anyhow::anyhow!("expected start event, got {:?}", rest)),
                };
                let tag_text = r.read_text(tag_start.name())?;
                match tag_start.local_name().as_ref() {
                    b"NewRemoteHost" => {
                        new_remote_host =
                            <RemoteHost as SVariable>::VarType::from_xml_value(&tag_text)
                                .map(Some)?
                    }
                    b"NewExternalPort" => {
                        new_external_port =
                            <ExternalPort as SVariable>::VarType::from_xml_value(&tag_text)
                                .map(Some)?
                    }
                    b"NewProtocol" => {
                        new_protocol =
                            <PortMappingProtocol as SVariable>::VarType::from_xml_value(&tag_text)
                                .map(Some)?
                    }
                    b"NewInternalPort" => {
                        new_internal_port =
                            <InternalPort as SVariable>::VarType::from_xml_value(&tag_text)
                                .map(Some)?
                    }
                    b"NewInternalClient" => {
                        new_internal_client =
                            <InternalClient as SVariable>::VarType::from_xml_value(&tag_text)
                                .map(Some)?
                    }
                    b"NewEnabled" => {
                        new_enabled =
                            <PortMappingEnabled as SVariable>::VarType::from_xml_value(&tag_text)
                                .map(Some)?
                    }
                    b"NewDescription" => {
                        new_description =
                            <PortMappingDescription as SVariable>::VarType::from_xml_value(
                                &tag_text,
                            )
                            .map(Some)?
                    }
                    b"NewLeaseTime" => {
                        new_lease_time =
                            <PortMappingLeaseDuration as SVariable>::VarType::from_xml_value(
                                &tag_text,
                            )
                            .map(Some)?
                    }
                    rest => {
                        tracing::trace!(
                            "enconutered unknown tag name: {}",
                            String::from_utf8_lossy(rest)
                        );
                    }
                }
                match r.read_event()? {
                    Event::End(bytes_end) if bytes_end == tag_start.to_end() => {}
                    rest => return Err(anyhow::anyhow!("expected end, got {:?}", rest)),
                }
            }

            let new_remote_host = new_remote_host.context("new remote host")?;
            let new_external_port = new_external_port.context("new external port")?;
            let new_protocol = new_protocol.context("new protocol")?;
            let new_internal_port = new_internal_port.context("new internal port")?;
            let new_internal_client = new_internal_client.context("new internal client")?;
            let new_enabled = new_enabled.context("new enabled")?;
            let new_description = new_description.context("new description")?;
            let new_lease_time = new_lease_time.context("new lease time")?;

            Ok(Self {
                new_remote_host,
                new_external_port,
                new_protocol,
                new_internal_port,
                new_internal_client,
                new_enabled,
                new_description,
                new_lease_time,
            })
        }
    }

    /// This argument type contains the list of port mapping entries.
    #[derive(Debug, Clone)]
    pub struct ArgPortListing {
        entries: Vec<PortMappingEntry>,
    }

    impl ArgPortListing {
        pub fn into_inner(self) -> Vec<PortMappingEntry> {
            self.entries
        }
    }

    impl IntoXml for ArgPortListing {
        fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
            let parent = BytesStart::new("p:PortMappingList").with_attributes([
                ("xmlns:p", "urn:schemas-upnp-org:gw:WANIPConnection"),
                ("xmlsn:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
                (
                    "xsi:schemaLocation",
                    "urn:schemas-upnp-org:gw:WANIPConnection
http://www.upnp.org/schemas/gw/WANIPConnection-v2.xsd",
                ),
            ]);
            w.write_event(Event::Start(parent.clone()))?;
            for entry in &self.entries {
                entry.write_xml(w)?;
            }
            w.write_event(Event::End(parent.to_end()))
        }
    }

    impl<'a> FromXml<'a> for ArgPortListing {
        fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self>
        where
            Self: Sized,
        {
            let parent = r.read_to_start()?.to_owned();
            anyhow::ensure!(parent.local_name().as_ref() == b"PortMappingList");

            let mut entries = Vec::new();

            while let Ok(entry) = PortMappingEntry::read_xml(r) {
                entries.push(entry);
            }

            match r.read_event()? {
                Event::End(end) => {
                    anyhow::ensure!(end == parent.to_end());
                }
                rest => {
                    return Err(anyhow::anyhow!(
                        "expected PortMappingList end, got {:?}",
                        rest
                    ));
                }
            }

            Ok(Self { entries })
        }
    }

    impl IntoUpnpValue for ArgPortListing {
        fn from_xml_value(value: &str) -> anyhow::Result<Self>
        where
            Self: Sized,
        {
            let mut reader = quick_xml::Reader::from_str(value);
            ArgPortListing::read_xml(&mut reader)
        }
    }

    impl SVariable for ArgPortListing {
        type VarType = Self;

        const VAR_NAME: &str = "A_ARG_TYPE_PortListing";
    }
}

// Client

/// Marker for [ScpdClient] that makes `WANIPConnection` actions implementation available
#[derive(Debug)]
pub struct InternetGatewayClient;

impl ScpdService for InternetGatewayClient {
    const URN: URN = URN {
        version: 1,
        urn_type: UrnType::Service(ServiceType::WANIPConnection),
    };
}

impl ScpdClient<InternetGatewayClient> {
    /// Like [add_port_mapping](ScpdClient::add_port_mapping) action, `AddAnyPortMapping` action also creates a port mapping specified with
    /// the same arguments.
    ///
    /// The behaviour differs only on the case where the specified port is not free, because in
    /// that case the gateway reserves any free `NewExternalPort` and `NewProtocol` pair and returns the
    /// `NewReservedPort`. It is up to the vendors to define an algorithm which finds a free port.
    /// It is encouraged to use this new action instead of the former one AddPortMapping() action, because it is
    /// more efficient, and it will be compatible with future potential solutions based on port range NAT solution
    /// also called "fractional address" within the IETF.
    ///
    /// The goal of "fractional address" NAT solution is to cope
    /// with the IPv4 public address exhaustion, by providing the same IPv4 public address to several IGDs, where
    /// each IGD is allocated with a different port range.
    ///
    /// NOTE: Not all NAT implementations will support:
    /// - Wildcard value (i.e. 0) for [ExternalPort],
    /// - InternalPort values that are different from [ExternalPort].
    ///
    /// Regarding the last point, this behaviour is not encouraged because the goal of `AddAnyPortMapping` is to
    /// provide a free port if the desired port is not free, so the [InternalPort] is potentially different from the
    /// [ExternalPort].
    ///
    /// Nevertheless, in order to be backward compatible with [AddPortMapping](ScpdClient::add_port_mapping) action, this
    /// behaviour is supported. If parameters `NewInternalClient`, `NewExternalPort` and `NewProtocol` are the same
    /// as an existing port mapping and control point is authorized for the operation, the port mapping is updated
    /// instead of creating new one.
    ///
    /// When a control point creates a port forwarding rule with `AddAnyPortMapping` for inbound traffic, this
    /// rule MUST also be applied when NAT port triggering occurs for outbound traffic.
    pub async fn add_any_port_mapping(
        &self,
        external_addr: Option<Ipv4Addr>,
        external_port: u16,
        proto: PortMappingProtocol,
        internal_port: u16,
        local_addr: Ipv4Addr,
        description: String,
        lease: u32,
    ) -> Result<u16, ActionCallError> {
        let action = self.action("AddAnyPortMapping")?;

        let payload = action.add_any_port_mapping(
            external_addr,
            external_port,
            proto,
            internal_port,
            local_addr,
            true,
            description,
            lease,
        )?;

        let port: u16 = self.run_action(action, payload).await?;
        Ok(port)
    }

    /// This action creates a new port mapping or overwrites an existing mapping with the same internal client. If
    /// the [ExternalPort] and [PortMappingProtocol] pair is already mapped to another internal client, an error is
    /// returned.
    pub async fn add_port_mapping(
        &self,
        external_addr: Option<Ipv4Addr>,
        external_port: u16,
        proto: PortMappingProtocol,
        internal_port: u16,
        local_addr: Ipv4Addr,
        description: String,
        lease: u32,
    ) -> Result<(), ActionCallError> {
        let action = self.action("AddPortMapping")?;

        let payload = action.add_port_mapping(
            external_addr,
            external_port,
            proto,
            internal_port,
            local_addr,
            true,
            description,
            lease,
        )?;

        () = self.run_action(action, payload).await?;
        Ok(())
    }

    /// This action deletes a previously instantiated port mapping.
    ///
    /// As each entry is deleted, the array is compacted,
    /// the evented variable [PortMappingNumberOfEntries] is decremented and the evented variable
    /// [SystemUpdateID] is incremented.
    pub async fn delete_port_mapping(
        &self,
        proto: PortMappingProtocol,
        external_port: u16,
    ) -> Result<(), ActionCallError> {
        let action = self.action("DeletePortMapping")?;

        let payload = action.remove_port_mapping(None, external_port, proto)?;

        () = self.run_action(action, payload).await?;
        Ok(())
    }

    /// This action retrieves the value of the external IP address on this connection instance.
    pub async fn get_external_ip_addr(&self) -> Result<Ipv4Addr, ActionCallError> {
        let action = self.action("GetExternalIPAddress")?;

        let payload = action.get_external_ip()?;

        let ip: Ipv4Addr = self.run_action(action, payload).await?;
        Ok(ip)
    }

    /// This action returns a list of port mappings matching the arguments.
    ///
    /// The operation of this action has two modes depending on `NewManage` value:
    /// - If the `NewManage` argument is set to "0" (false), then this action returns a list of port mappings
    /// that have [InternalClient] value matching to the IP address of the control point between
    /// `NewStartPort` and `NewEndPort`,
    /// - If the NewManage argument is set to "1" (true), then the gateway MUST return all port mappings
    /// between NewStartPort and NewEndPort.
    ///
    /// With the argument `NewNumberOfPorts` (take), a control point MAY limit the size of the list returned in order to
    /// limit the length of the list returned. If NewNumberOfPorts is equal to 0, then the gateway MUST return all
    /// port mappings between `NewStartPort` and `NewEndPort`.
    ///
    /// The returned port mappings also depends on the authentication of the control point.
    pub async fn list_all_port_mappings(
        &self,
        port_start: u16,
        port_end: u16,
        protocol: PortMappingProtocol,
        manage: bool,
        take: u32,
    ) -> Result<Vec<PortMappingEntry>, ActionCallError> {
        let action = self.action("GetListOfPortMappings")?;

        let payload =
            action.get_list_of_port_mappings(port_start, port_end, protocol, manage, take)?;

        let mappings: ArgPortListing = self.run_action(action, payload).await?;
        Ok(mappings.into_inner())
    }
}
