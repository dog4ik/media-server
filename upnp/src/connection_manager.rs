use quick_xml::events::{BytesText, Event};

use crate::{
    action::{Action, ActionError, IntoValueList},
    service::{ArgumentScanner, Service},
    service_variables::{IntoUpnpValue, SVariable, StateVariableDescriptor},
    templates::{service_description::ServiceDescription, SpecVersion},
    urn, IntoXml,
};

#[allow(unused)]
pub trait ConnectionManagerHandler {
    /// This REQUIRED action returns the protocol-related info that this ConnectionManager supports in its
    /// current state, as a Comma-Separated Value list of strings according to Table 2-20, “Defined Protocols and
    /// their associated ProtocolInfo ”. Protocol-related information for ‘sourcing’ data is returned in the Source
    /// argument and protocol-related information for ‘sinking’ data is returned in the Sink argument. When this
    /// ConnectionManager resides in a device that only supports ‘sourcing’ of data, the Sink argument MUST
    /// return the empty string. Likewise, when this ConnectionManager resides in a device that only supports
    /// ‘sinking’ of data, the Source argument MUST return the empty string.
    fn get_protocol_info(
        &self,
    ) -> impl std::future::Future<Output = Result<(String, String), ActionError>> + Send;

    fn get_current_connection_ids(
        &self,
    ) -> impl std::future::Future<Output = Result<String, ActionError>> + Send + Sync;

    fn get_current_connection_info(
        &self,
        connection_id: String,
    ) -> impl std::future::Future<
        Output = Result<(String, String, String, String, Direction, String), ActionError>,
    > + Send
           + Sync;

    fn get_feature_list(
        &self,
    ) -> impl std::future::Future<Output = Result<String, ActionError>> + Send + Sync;

    fn get_renderer_item_info(
        &self,
        item_info_filter: String,
        item_metadata_list: String,
    ) -> impl std::future::Future<Output = Result<String, ActionError>> + Send + Sync {
        async { Err(ActionError::not_implemented()) }
    }

    /// This OPTIONAL action is used to allow the device to prepare itself to connect to the network for the
    /// purposes of sending or receiving media content (for example, a video stream). PrepareForConnection()
    /// also allows the device to indicate whether or not it can establish a connection based on the current status
    /// of the device and/or the current conditions of the network.
    /// The RemoteProtocolInfo input argument identifies the protocol, network, and format that MUST be used
    /// to transfer the content.
    ///
    /// - If PrepareForConnection() is invoked on a MediaServer device, the RemoteProtocolInfo
    /// argument MUST be set to one of the ProtocolInfo entries from the CSV list obtained from the
    /// peer MediaRenderer device via the GetProtocolInfo() action. (See Section 2.5.2, “ProtocolInfo
    /// Concept” for details.) If the peer device does not implement GetProtocolInfo() (because it is not a
    /// MediaRenderer or not even a UPnP device), then the RemoteProtocolInfo argument MUST be set
    /// to one of the ProtocolInfo entries returned by the GetProtocolInfo() action on the local
    /// MediaServer device.
    ///
    /// - If PrepareForConnection() is invoked on a MediaRenderer device, the RemoteProtocolInfo
    /// argument MUST be set to the value of the protocolInfo attribute of the content item (located in
    /// the ContentDirectory on the peer MediaServer device) that is going to be played. (See Section
    /// 2.5.2, “ProtocolInfo Concept” for details.) If the peer device does not implement a
    /// ContentDirectory service (because it is not a MediaServer or not even a UPnP device), then the
    /// RemoteProtocolInfo argument MUST be set to one of the ProtocolInfo entries returned by the
    /// GetProtocolInfo() action on the local MediaRenderer device.
    ///
    /// The ConnectionID out argument is used to identify the connection that was prepared by the device in
    /// response to this invocation. The ConnectionID is a device-specific value and is NOT unique throughout
    /// the network. Therefore, the ConnectionIDs returned by the two end-points of the same connection will
    /// generally NOT be the same value. Refer to GetCurrentConnectionIDs() and/or the UPnP AV Device
    /// Architecture document for additional information. The AVTransportID and RcsID out arguments are used
    /// to identify the AVTransport and RenderingControl services that are associated with the connection. The
    /// returned values are the InstanceIDs that need to be used when invoking subsequent invocations of the
    /// AVTransport and RenderingControl Services. An InstanceID value of -1 indicates the device did not
    /// associate an AVTransport and/or RenderingControl service with this connection. The returned
    /// ConnectionID, AVTransportID, and RcsID become invalid when the device closes the connection. This
    /// will occur when ConnectionComplete() is invoked or any other time the device decides to close the
    /// connection (a.k.a auto-cleanup). Refer to ConnectionComplete() for additional information.
    ///
    /// This action is marked OPTIONAL which means that each device manufacturer decides whether or not to
    /// implement it. Therefore, some devices will implement PrepareForConnection() while other devices will
    /// not. Since PrepareForConnection() allows a device to prepare itself to connect to the network, if a device
    /// has implemented that action, control points need to invoke PrepareForConnection() before attempting to
    /// stream any content; that is: before invoking AVTransport::SetAVTransportURI() (See Section 2.5.3,
    /// “Typical Control Point Operations”). Otherwise, the device may not operate correctly because it has not
    /// been properly configured. Additionally, control points need to invoke PrepareForConnection(), if
    /// implemented, so that the device can inform the control point, via an error code, that the device’s current
    /// operating environment is not able to accommodate the requested stream.
    /// Once a connection has been prepared, it can be used to transfer several pieces of the content before calling
    /// ConnectionComplete() as long as each content item is compatible with the RemoteProtocolInfo argument
    /// that was passed into PrepareForConnection(); that is: each content item has the same media format as
    /// specified in RemoteprotocolInfo.
    ///
    /// If a device does not implement PrepareForConnection(), it MUST only support a single connection at any
    /// time. This connection is implicitly assumed to be always present and is identified by ConnectionID = 0.
    fn prepare_for_connection(
        &self,
        remote_protocol_info: String,
        peer_connection_manager: String,
        connection_id: String,
        direction: Direction,
    ) -> impl std::future::Future<
        Output = Result<(String, Option<String>, Option<String>), ActionError>,
    > + Send
           + Sync {
        async { Err(ActionError::not_implemented()) }
    }

    /// This OPTIONAL action is used to inform the device that the specified connection, which was previously
    /// allocated by PrepareForConnection(), is no longer needed. Any resources that were allocated for that
    /// connection during PrepareForConnection() can be freed by the device at its discretion.
    /// In some situations, ConnectionComplete() may never be invoked; for example, the control point
    /// spontaneously goes away. In order to prevent an unused connection from permanently consuming
    /// resources, the device SHOULD automatically cleanup unused connections. The process for determining
    /// when a connection SHOULD be automatically cleaned up is implementation dependent. For example, a
    /// device MAY decide to close a connection after the connection has been inactive for a certain period of
    /// time. Alternatively, a device MAY decide to close a connection when it needs to free the resources that are
    /// associated with the connection.
    fn connection_complete(
        &self,
        connection_id: String,
    ) -> impl std::future::Future<Output = Result<(), ActionError>> + Send + Sync {
        async { Err(ActionError::not_implemented()) }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionManagerService<T: ConnectionManagerHandler> {
    pub handler: T,
}

impl<T: ConnectionManagerHandler> ConnectionManagerService<T> {
    pub fn new(handler: T) -> Self {
        Self { handler }
    }

    async fn get_protocol_info(&self) -> Result<(String, String), ActionError> {
        todo!()
    }

    async fn get_current_connection_ids(&self) -> Result<String, ActionError> {
        todo!()
    }

    async fn get_current_connection_info(
        &self,
        _connection_id: String,
    ) -> Result<(i32, i32, String, String, Direction, ConnectionStatus), ActionError> {
        todo!()
    }

    async fn get_feature_list(&self) -> Result<String, ActionError> {
        todo!()
    }

    async fn get_renderer_item_info(
        &self,
        _item_info_filter: String,
        _item_metadata_list: String,
    ) -> Result<String, ActionError> {
        todo!()
    }

    async fn prepare_for_connection(
        &self,
        _remote_protocol_info: String,
        _peer_connection_manager: String,
        _connection_id: i32,
        _direction: Direction,
    ) -> Result<(String, i32, i32), ActionError> {
        todo!()
    }

    async fn connection_complete(&self, _connection_id: String) -> Result<(), ActionError> {
        todo!()
    }
}

impl<T: ConnectionManagerHandler + Send + Sync + 'static> Service for ConnectionManagerService<T> {
    const NAME: &str = "connection_manager";

    const URN: urn::URN = urn::URN {
        version: 3,
        urn_type: urn::UrnType::Service(urn::ServiceType::ConnectionManager),
    };

    fn service_description() -> ServiceDescription {
        let variables = vec![
            StateVariableDescriptor::from_variable::<SourceProtocolInfo>(),
            StateVariableDescriptor::from_variable::<SinkProtocolInfo>(),
            StateVariableDescriptor::from_variable::<CurrentConnectionIDs>(),
            StateVariableDescriptor::from_variable::<FeatureList>(),
            StateVariableDescriptor::from_variable::<ClockUpdateID>(),
            StateVariableDescriptor::from_variable::<DeviceClockInfoUpdates>(),
            StateVariableDescriptor::from_variable::<ConnectionStatus>(),
            StateVariableDescriptor::from_variable::<ConnectionManager>(),
            StateVariableDescriptor::from_variable::<Direction>(),
            StateVariableDescriptor::from_variable::<ProtocolInfo>(),
            StateVariableDescriptor::from_variable::<ConnectionID>(),
            StateVariableDescriptor::from_variable::<AVTransportID>(),
            StateVariableDescriptor::from_variable::<RcsID>(),
            StateVariableDescriptor::from_variable::<ItemInfoFilter>(),
            StateVariableDescriptor::from_variable::<ArgResult>(),
            StateVariableDescriptor::from_variable::<RenderingInfoList>(),
        ];
        ServiceDescription {
            spec_version: SpecVersion::upnp_v2(),
            variables,
            actions: Self::actions(),
        }
    }

    fn actions() -> Vec<Action> {
        let mut get_protocol_info = Action::empty("GetProtocolInfo");
        get_protocol_info.add_output::<SourceProtocolInfo>("Source");
        get_protocol_info.add_output::<SinkProtocolInfo>("Sink");
        let mut prepare_for_connection = Action::empty("PrepareForConnection");
        prepare_for_connection.add_input::<ProtocolInfo>("RemoteProtocolInfo");
        prepare_for_connection.add_input::<ConnectionManager>("PeerConnectionManager");
        prepare_for_connection.add_input::<ConnectionID>("PeerConnectionID");
        prepare_for_connection.add_input::<Direction>("Direction");
        prepare_for_connection.add_output::<ConnectionID>("ConnectionID");
        prepare_for_connection.add_output::<AVTransportID>("AVTransportID");
        prepare_for_connection.add_output::<RcsID>("RcsID");
        let mut connection_complete = Action::empty("ConnectionComplete");
        connection_complete.add_input::<ConnectionID>("ConnectionID");
        let mut current_connection_ids = Action::empty("GetCurrentConnectionIDs");
        current_connection_ids.add_output::<CurrentConnectionIDs>("ConnectionIDs");
        let mut current_connection_info = Action::empty("GetCurrentConnectionInfo");
        current_connection_info.add_input::<ConnectionID>("ConnectionID");
        current_connection_info.add_output::<RcsID>("RcsID");
        current_connection_info.add_output::<AVTransportID>("AVTransportID");
        current_connection_info.add_output::<ProtocolInfo>("ProtocolInfo");
        current_connection_info.add_output::<ConnectionManager>("PeerConnectionManager");
        current_connection_info.add_output::<ConnectionID>("PeerConnectionID");
        current_connection_info.add_output::<Direction>("Direction");
        current_connection_info.add_output::<ConnectionStatus>("Status");
        let mut renderer_item_info = Action::empty("GetRendererItemInfo");
        renderer_item_info.add_input::<ItemInfoFilter>("ItemInfoFilter");
        renderer_item_info.add_input::<ArgResult>("ItemMetadataList");
        renderer_item_info.add_output::<RenderingInfoList>("ItemRenderingInfoList");
        let mut get_feature_list = Action::empty("GetFeatureList");
        get_feature_list.add_output::<FeatureList>("FeatureList");

        vec![
            get_protocol_info,
            prepare_for_connection,
            connection_complete,
            current_connection_ids,
            current_connection_info,
            renderer_item_info,
            get_feature_list,
        ]
    }

    async fn control_handler<'a>(
        &self,
        name: &'a str,
        mut inputs: ArgumentScanner<'a>,
    ) -> anyhow::Result<impl crate::action::IntoValueList> {
        tracing::debug!("Got connection manager action {}", name);
        match name {
            "GetProtocolInfo" => Ok(self.get_protocol_info().await?.into_value_list()),
            "PrepareForConnection" => Ok(self
                .prepare_for_connection(
                    inputs.next()?,
                    inputs.next()?,
                    inputs.next()?,
                    inputs.next()?,
                )
                .await?
                .into_value_list()),
            "ConnectionComplete" => Ok(self
                .connection_complete(inputs.next()?)
                .await?
                .into_value_list()),
            "GetCurrentConnectionIDs" => {
                Ok(self.get_current_connection_ids().await?.into_value_list())
            }
            "GetCurrentConnectionInfo" => Ok(self
                .get_current_connection_info(inputs.next()?)
                .await?
                .into_value_list()),
            "GetRendererItemInfo" => Ok(self
                .get_renderer_item_info(inputs.next()?, inputs.next()?)
                .await?
                .into_value_list()),
            "GetFeatureList" => Ok(self.get_feature_list().await?.into_value_list()),
            rest => Err(anyhow::anyhow!("unhandled action: {rest}")),
        }
    }
}

/// This REQUIRED state variable contains a Comma-Separated Value (CSV) list of information on
/// protocols this ConnectionManager supports for ‘sourcing’ (sending) data, in its current state. (The content
/// of the CSV list can change over time, for example due to local resource restrictions on the device.) Besides
/// the traditional notion of the term ‘protocol’, the protocol-related information provided by the connection
/// also contains other information such as supported content formats. See Section 2.5, “Theory of Operation”
/// for a general discussion on the notion of protocol info. See the table in Section 2.5.2, “ProtocolInfo
/// Concept” for specific allowed values for this state variable. If the device does not support sourcing data,
/// this state variable MUST be set to the empty string.
/// During normal operation, a MediaServer SHOULD ensure that there is consistency between what is
/// reported in the SourceProtocolInfo state variable and all the res@protocolInfo properties of the items that
/// populate the ContentDirectory; that is: at least all protocols that are used by any of the content items
/// SHOULD be enumerated in the SourceProtocolInfo state variable. (Wildcards (“*”) can be used in
/// SourceProtocolInfo to limit the number of entries in the CSV list.) Additional protocols that are supported
/// by the MediaServer but are not currently used by any of the content items MAY also be listed.
/// Control points can use the SourceProtocolInfo CSV list to quickly find out what type of content this
/// MediaServer is capable of serving to the network.
#[derive(Default, Debug)]
struct SourceProtocolInfo;
impl SVariable for SourceProtocolInfo {
    type VarType = String;
    const VAR_NAME: &str = "SourceProtocolInfo";
}

/// This REQUIRED state variable contains a Comma-Separated Value (CSV) list of information on
/// protocols this ConnectionManager supports for ‘sinking’ (receiving) data, in its current state. (The
/// content of the CSV list can change over time, for example due to local resource restrictions on the device.)
/// The format and allowed value list are the same as for the SourceProtocolInfo state variable. If the device
/// does not support ‘sinking’ data, this state variable MUST be set to the empty string.
/// A MediaRenderer can report temporary unavailability of a protocol (for example, codec not available) by
/// removing the appropriate entries from the SinkProtocolInfo CSV list.
#[derive(Default, Debug)]
struct SinkProtocolInfo;
impl SVariable for SinkProtocolInfo {
    type VarType = String;
    const VAR_NAME: &str = "SinkProtocolInfo";
}

/// This REQUIRED state variable contains a Comma-Separated Value list of references to current active
/// Connections. This list MAY change without explicit actions invoked by control points, for example by
/// out-of-band cleanup or termination of finished connections.
/// If OPTIONAL action PrepareForConnection() is not implemented then this state variable MUST be set to
/// “0”, indicating that this ConnectionManager service only supports a single connection identified by
/// ConnectionID = 0.
#[derive(Default, Debug)]
struct CurrentConnectionIDs;
impl SVariable for CurrentConnectionIDs {
    type VarType = String;
    const VAR_NAME: &str = "CurrentConnectionIDs";
}

/// This REQUIRED state variable enumerates the CM features (see Appendix B) supported by this
/// ConnectionManager service. The value is a valid Features XML Document, according to [CM-FTRLST-
/// XSD].
/// - The root element of the document is <Features>. It contains zero or more child <Feature>
/// elements, each of which represents one ConnectionManager service feature that is supported in
/// this implementation.
/// - A <Feature> element MUST have a version attribute and MUST have a name attribute
/// containing the assigned name of the feature.
/// - A <Feature> element MAY have other attributes defined per each feature.
/// See the schema in [CM-FTRLST-XSD] for more details on the structure.
#[derive(Default, Debug)]
struct FeatureList;
impl SVariable for FeatureList {
    type VarType = String;
    const VAR_NAME: &str = "FeatureList";
}

/// This CONDITIONALLY REQUIRED state variable MUST be supported if the CLOCKSYNC feature is
/// implemented. It is used to identify the current instance of the CLOCKSYNC feature. This state variable is
/// modified whenever a change occurs in the CLOCKSYNC feature of the device. A change can be an
/// addition or modification to the <DeviceClockInfo> element of the CLOCKSYNC feature. The
/// ClockUpdateID state variable contains a numeric value that is incremented whenever change occurs in
/// CLOCKSYNC feature of the device. Initial value of ClockUpdateID state variable MUST be zero (0).
/// #[derive(Default, Debug)]
struct ClockUpdateID;
impl SVariable for ClockUpdateID {
    type VarType = u32;
    const VAR_NAME: &str = "ClockUpdateID";
}

#[derive(Default, Debug)]
struct DeviceClockInfoUpdates;
impl SVariable for DeviceClockInfoUpdates {
    type VarType = String;
    const VAR_NAME: &str = "DeviceClockInfoUpdates";
}

#[derive(Debug)]
enum ConnectionStatus {
    Ok,
    ContentFormatMismatch,
    InsufficienBandwidth,
    UnreliableChannel,
    Unknown,
}

impl IntoUpnpValue for ConnectionStatus {
    const TYPE_NAME: crate::service_variables::DataType =
        crate::service_variables::DataType::String;

    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match value {
            "OK" => Ok(Self::Ok),
            "ContentFormatMismatch" => Ok(Self::ContentFormatMismatch),
            "InsufficienBandwidth" => Ok(Self::InsufficienBandwidth),
            "UnreliableChannel" => Ok(Self::UnreliableChannel),
            "Unknown" => Ok(Self::Unknown),
            _ => Err(anyhow::anyhow!("unknown ConnectionStatus value: {value}")),
        }
    }
}

impl IntoXml for ConnectionStatus {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let val = match self {
            ConnectionStatus::Ok => "OK",
            ConnectionStatus::ContentFormatMismatch => "ContentFormatMismatch",
            ConnectionStatus::InsufficienBandwidth => "InsufficienBandwidth",
            ConnectionStatus::UnreliableChannel => "UnreliableChannel",
            ConnectionStatus::Unknown => "Unknown",
        };
        w.write_event(Event::Text(BytesText::new(val)))
    }
}

impl SVariable for ConnectionStatus {
    type VarType = Self;
    const ALLOWED_VALUE_LIST: Option<&[&str]> = Some(&[
        "OK",
        "ContentFormatMismatch",
        "InsufficienBandwidth",
        "UnreliableChannel",
        "Unknown",
    ]);
    const VAR_NAME: &str = "ConnectionStatus";
}

#[derive(Default, Debug)]
struct ConnectionManager;
impl SVariable for ConnectionManager {
    type VarType = String;
    const VAR_NAME: &str = "ConnectionManager";
}

#[derive(Debug)]
pub enum Direction {
    Output,
    Input,
}

impl IntoUpnpValue for Direction {
    const TYPE_NAME: crate::service_variables::DataType =
        crate::service_variables::DataType::String;

    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        match value {
            "Input" => Ok(Self::Input),
            "Output" => Ok(Self::Output),
            _ => Err(anyhow::anyhow!("expected Input or Output, got {value}")),
        }
    }
}

impl IntoXml for Direction {
    fn write_xml(&self, w: &mut crate::XmlWriter) -> std::io::Result<()> {
        let msg = match self {
            Direction::Output => "Output",
            Direction::Input => "Input",
        };
        w.write_event(Event::Text(BytesText::new(msg)))
    }
}

impl SVariable for Direction {
    type VarType = Self;
    const VAR_NAME: &str = "Direction";
}

#[derive(Default, Debug)]
struct ProtocolInfo;
impl SVariable for ProtocolInfo {
    type VarType = String;
    const VAR_NAME: &str = "ProtocolInfo";
}

#[derive(Default, Debug)]
struct ConnectionID;
impl SVariable for ConnectionID {
    type VarType = i32;
    const VAR_NAME: &str = "ConnectionID";
}

#[derive(Default, Debug)]
struct AVTransportID;
impl SVariable for AVTransportID {
    type VarType = i32;
    const VAR_NAME: &str = "AVTransportID";
}

#[derive(Default, Debug)]
struct RcsID;
impl SVariable for RcsID {
    type VarType = i32;
    const VAR_NAME: &str = "RcsID";
}

#[derive(Default, Debug)]
struct ItemInfoFilter;
impl SVariable for ItemInfoFilter {
    type VarType = String;
    const VAR_NAME: &str = "ItemInfoFilter";
}

#[derive(Default, Debug)]
struct ArgResult;
impl SVariable for ArgResult {
    type VarType = String;
    const VAR_NAME: &str = "Result";
}

#[derive(Default, Debug)]
struct RenderingInfoList;
impl SVariable for RenderingInfoList {
    type VarType = String;
    const VAR_NAME: &str = "RenderingInfoList";
}
