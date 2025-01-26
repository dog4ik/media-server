use std::net::Ipv4Addr;

use port_listing::{ArgPortListing, PortMappingEntry};
use quick_xml::events::{BytesText, Event};

use crate::{
    service_client::{ActionCallError, ScpdClient, ScpdService},
    service_variables::{IntoUpnpValue, SVariable},
    urn::{ServiceType, UrnType, URN},
    IntoXml,
};

#[derive(Default, Debug, PartialEq, Eq, Clone, Copy)]
enum ConnectionType {
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

#[derive(Debug)]
pub struct PossibleConnectionTypes;

impl SVariable for PossibleConnectionTypes {
    type VarType = String;

    const VAR_NAME: &str = "PossibleConnectionTypes";
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ConnectionStatus {
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
enum LastConnectionError {
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

#[derive(Debug)]
pub struct AutoDisconnectTime;
impl SVariable for AutoDisconnectTime {
    type VarType = u32;

    const VAR_NAME: &str = "AutoDisconnectTime";
}

#[derive(Debug)]
pub struct IdleDisconnectTime;
impl SVariable for IdleDisconnectTime {
    type VarType = u32;

    const VAR_NAME: &str = "IdleDisconnectTime";
}

#[derive(Debug)]
pub struct WarnDisconnectDelay;
impl SVariable for WarnDisconnectDelay {
    type VarType = u32;

    const VAR_NAME: &str = "WarnDisconnectDelay";
}

#[derive(Debug)]
pub struct RSIPAvailable;
impl SVariable for RSIPAvailable {
    type VarType = bool;

    const VAR_NAME: &str = "RSIPAvailable";
}

#[derive(Debug)]
pub struct NATEnabled;
impl SVariable for NATEnabled {
    type VarType = bool;

    const VAR_NAME: &str = "NATEnabled";
}

#[derive(Debug)]
pub struct ExternalIPAddress;
impl SVariable for ExternalIPAddress {
    type VarType = Ipv4Addr;

    const VAR_NAME: &str = "ExternalIPAddress";
}

#[derive(Debug)]
pub struct PortMappingNumberOfEntries;
impl SVariable for PortMappingNumberOfEntries {
    type VarType = u32;

    const VAR_NAME: &str = "PortMappingNumberOfEntries";
}

#[derive(Debug)]
pub struct PortMappingEnabled;
impl SVariable for PortMappingEnabled {
    type VarType = bool;

    const VAR_NAME: &str = "PortMappingEnabled";
}

#[derive(Debug)]
pub struct PortMappingLeaseDuration;
impl SVariable for PortMappingLeaseDuration {
    type VarType = u32;
    const RANGE: Option<crate::service_variables::Range> = Some(crate::service_variables::Range {
        start: 0,
        end: 604800,
        step: None,
    });

    const VAR_NAME: &str = "PortMappingLeaseDuration";
}

#[derive(Debug)]
pub struct RemoteHost;

impl SVariable for RemoteHost {
    type VarType = Option<Ipv4Addr>;

    const VAR_NAME: &str = "RemoteHost";
}

#[derive(Debug)]
pub struct ExternalPort;
impl SVariable for ExternalPort {
    type VarType = u16;

    const VAR_NAME: &str = "ExternalPort";
}

#[derive(Debug)]
pub struct InternalPort;
impl SVariable for InternalPort {
    type VarType = u16;

    const VAR_NAME: &str = "InternalPort";
}

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

    const VAR_NAME: &str = "PortMappingProtocol";
}

#[derive(Debug)]
pub struct InternalClient;
impl SVariable for InternalClient {
    type VarType = Ipv4Addr;

    const VAR_NAME: &str = "InternalClient";
}

#[derive(Debug)]
pub struct PortMappingDescription;
impl SVariable for PortMappingDescription {
    type VarType = String;

    const VAR_NAME: &str = "PortMappingDescription";
}

#[derive(Debug)]
struct SystemUpdateID;
impl SVariable for SystemUpdateID {
    type VarType = u32;

    const VAR_NAME: &str = "SystemUpdateID";
}

#[derive(Debug)]
pub struct ArgManage;
impl SVariable for ArgManage {
    type VarType = bool;

    const VAR_NAME: &str = "A_ARG_TYPE_Manage";
}

mod port_listing {

    use anyhow::Context;
    use quick_xml::events::{BytesStart, Event};

    use crate::{
        service_variables::{IntoUpnpValue, SVariable},
        FromXml, IntoXml, XmlReaderExt,
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
                    ))
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

#[derive(Debug)]
pub struct InternetGatewayClient;

impl ScpdService for InternetGatewayClient {
    const URN: URN = URN {
        version: 1,
        urn_type: UrnType::Service(ServiceType::WANIPConnection),
    };
}

impl ScpdClient<InternetGatewayClient> {
    pub async fn add_any_port_mapping(
        &self,
        local_addr: Ipv4Addr,
        external_addr: Option<Ipv4Addr>,
        proto: PortMappingProtocol,
        description: String,
        external_port: u16,
        lease: u32,
    ) -> Result<u16, ActionCallError> {
        let action = self.action("AddAnyPortMapping")?;

        let payload = action.add_port_mapping(
            external_addr,
            external_port,
            proto,
            external_port,
            local_addr,
            true,
            description,
            lease,
        )?;

        let port: u16 = self.run_action(action, payload).await?;
        Ok(port)
    }

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

    pub async fn get_external_ip_addr(&self) -> Result<Ipv4Addr, ActionCallError> {
        let action = self.action("GetExternalIPAddress")?;

        let payload = action.get_external_ip()?;

        let ip: Ipv4Addr = self.run_action(action, payload).await?;
        Ok(ip)
    }

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
