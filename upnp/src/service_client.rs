use std::{fmt::Display, marker::PhantomData};

use crate::{
    action::{
        ActionError, ActionResponse, ArgumentDirection, InArgumentPayload, ScannableArguments,
        SoapMessage, WritableAction,
    },
    av_transport::{ArgInstanceID, ArgSeekMode, ArgSeekTarget},
    internet_gateway::{
        ArgManage, ExternalPort, InternalClient, InternalPort, PortMappingDescription,
        PortMappingEnabled, PortMappingLeaseDuration, PortMappingNumberOfEntries,
        PortMappingProtocol, RemoteHost,
    },
    service::ArgumentScanner,
    service_variables::SVariable,
    templates::service_description::Scpd,
    urn::{ServiceType, UrnType, URN},
    FromXml,
};

#[derive(Debug)]
pub struct Action {
    name: String,
    pub in_args: Vec<String>,
    pub out_args: Vec<String>,
}

impl Action {
    const WANIPCONNECTION_URN: URN = URN {
        version: 1,
        urn_type: UrnType::Service(ServiceType::WANIPConnection),
    };
    const AVTRANSPORT_URN: URN = URN {
        version: 1,
        urn_type: UrnType::Service(ServiceType::AVTransport),
    };

    pub fn av_play(
        &self,
        instance_id: <ArgInstanceID as SVariable>::VarType,
        speed: &str,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("Play", Self::AVTRANSPORT_URN)?;
        for argument in &self.in_args {
            match argument.as_str() {
                "InstanceID" => action.write_argument(argument, instance_id.as_str()),
                "Speed" => action.write_argument(argument, speed),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }

    pub fn av_pause(
        &self,
        instance_id: <ArgInstanceID as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("Pause", Self::AVTRANSPORT_URN)?;
        for argument in &self.in_args {
            match argument.as_str() {
                "InstanceID" => action.write_argument(argument, instance_id.as_str()),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }

    pub fn av_seek(
        &self,
        instance_id: <ArgInstanceID as SVariable>::VarType,
        unit: <ArgSeekMode as SVariable>::VarType,
        target: <ArgSeekTarget as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("Seek", Self::AVTRANSPORT_URN)?;
        for argument in &self.in_args {
            match argument.as_str() {
                "InstanceID" => action.write_argument(argument, instance_id.as_str()),
                "Unit" => action.write_argument(argument, unit),
                "Target" => action.write_argument(argument, target.as_str()),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }

    pub fn av_position_info(
        &self,
        instance_id: <ArgInstanceID as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("GetPositionInfo", Self::AVTRANSPORT_URN)?;
        for argument in &self.in_args {
            match argument.as_str() {
                "InstanceID" => action.write_argument(argument, instance_id.as_str()),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }

    pub fn add_port_mapping(
        &self,
        remote_host: <RemoteHost as SVariable>::VarType,
        external_port: <ExternalPort as SVariable>::VarType,
        new_protocol: <PortMappingProtocol as SVariable>::VarType,
        internal_port: <InternalPort as SVariable>::VarType,
        internal_client: <InternalClient as SVariable>::VarType,
        enabled: <PortMappingEnabled as SVariable>::VarType,
        description: <PortMappingDescription as SVariable>::VarType,
        lease_duration: <PortMappingLeaseDuration as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("AddPortMapping", Self::WANIPCONNECTION_URN)?;
        for argument in &self.in_args {
            match argument.as_str() {
                "NewRemoteHost" => action.write_argument(argument, remote_host),
                "NewExternalPort" => action.write_argument(argument, external_port),
                "NewProtocol" => action.write_argument(argument, new_protocol),
                "NewInternalPort" => action.write_argument(argument, internal_port),
                "NewInternalClient" => action.write_argument(argument, internal_client),
                "NewEnabled" => action.write_argument(argument, enabled),
                "NewPortMappingDescription" => {
                    action.write_argument(argument, description.as_str())
                }
                "NewLeaseDuration" => action.write_argument(argument, lease_duration),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }


    pub fn add_any_port_mapping(
        &self,
        remote_host: <RemoteHost as SVariable>::VarType,
        external_port: <ExternalPort as SVariable>::VarType,
        new_protocol: <PortMappingProtocol as SVariable>::VarType,
        internal_port: <InternalPort as SVariable>::VarType,
        internal_client: <InternalClient as SVariable>::VarType,
        enabled: <PortMappingEnabled as SVariable>::VarType,
        description: <PortMappingDescription as SVariable>::VarType,
        lease_duration: <PortMappingLeaseDuration as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("AddAnyPortMapping", Self::WANIPCONNECTION_URN)?;
        for argument in &self.in_args {
            match argument.as_str() {
                "NewRemoteHost" => action.write_argument(argument, remote_host),
                "NewExternalPort" => action.write_argument(argument, external_port),
                "NewProtocol" => action.write_argument(argument, new_protocol),
                "NewInternalPort" => action.write_argument(argument, internal_port),
                "NewInternalClient" => action.write_argument(argument, internal_client),
                "NewEnabled" => action.write_argument(argument, enabled),
                "NewPortMappingDescription" => {
                    action.write_argument(argument, description.as_str())
                }
                "NewLeaseDuration" => action.write_argument(argument, lease_duration),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }

    pub fn add_any_port_mapping_strict(
        &self,
        remote_host: <RemoteHost as SVariable>::VarType,
        external_port: <ExternalPort as SVariable>::VarType,
        new_protocol: <PortMappingProtocol as SVariable>::VarType,
        internal_port: <InternalPort as SVariable>::VarType,
        internal_client: <InternalClient as SVariable>::VarType,
        enabled: <PortMappingEnabled as SVariable>::VarType,
        description: <PortMappingDescription as SVariable>::VarType,
        lease_duration: <PortMappingLeaseDuration as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("AddAnyPortMapping", Self::WANIPCONNECTION_URN)?;

        let mut expected = self.in_args.iter().map(|s| s.as_str());
        // Order is important!

        anyhow::ensure!(expected.next() == Some("NewRemoteHost"));
        action.write_argument("NewRemoteHost", remote_host)?;

        anyhow::ensure!(expected.next() == Some("NewExternalPort"));
        action.write_argument("NewExternalPort", external_port)?;

        anyhow::ensure!(expected.next() == Some("NewProtocol"));
        action.write_argument("NewProtocol", new_protocol)?;

        anyhow::ensure!(expected.next() == Some("NewInternalPort"));
        action.write_argument("NewInternalPort", internal_port)?;

        anyhow::ensure!(expected.next() == Some("NewInternalClient"));
        action.write_argument("NewInternalClient", internal_client)?;

        anyhow::ensure!(expected.next() == Some("NewEnabled"));
        action.write_argument("NewEnabled", enabled)?;

        anyhow::ensure!(expected.next() == Some("NewPortMappingDescription"));
        action.write_argument("NewPortMappingDescription", description)?;

        anyhow::ensure!(expected.next() == Some("NewLeaseDuration"));
        action.write_argument("NewLeaseDuration", lease_duration)?;

        Ok(action.finish()?)
    }

    pub fn remove_port_mapping(
        &self,
        remote_host: <RemoteHost as SVariable>::VarType,
        external_port: <ExternalPort as SVariable>::VarType,
        new_protocol: <PortMappingProtocol as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("DeletePortMapping", Self::WANIPCONNECTION_URN)?;
        for argument in &self.in_args {
            match argument.as_str() {
                "NewRemoteHost" => action.write_argument(argument, remote_host),
                "NewExternalPort" => action.write_argument(argument, external_port),
                "NewProtocol" => action.write_argument(argument, new_protocol),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }

    pub fn remove_port_mapping_strict(
        &self,
        remote_host: <RemoteHost as SVariable>::VarType,
        external_port: <ExternalPort as SVariable>::VarType,
        new_protocol: <PortMappingProtocol as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("DeletePortMapping", Self::WANIPCONNECTION_URN)?;

        let mut expected = self.in_args.iter().map(|s| s.as_str());
        // Order is important!

        anyhow::ensure!(expected.next() == Some("NewRemoteHost"));
        action.write_argument("NewRemoteHost", remote_host)?;

        anyhow::ensure!(expected.next() == Some("NewExternalPort"));
        action.write_argument("NewExternalPort", external_port)?;

        anyhow::ensure!(expected.next() == Some("NewProtocol"));
        action.write_argument("NewProtocol", new_protocol)?;

        Ok(action.finish()?)
    }

    pub fn get_external_ip(&self) -> anyhow::Result<String> {
        let action = WritableAction::new("GetExternalIPAddress", Self::WANIPCONNECTION_URN)?;
        Ok(action.finish()?)
    }

    pub fn get_list_of_port_mappings(
        &self,
        start_port: <ExternalPort as SVariable>::VarType,
        end_port: <ExternalPort as SVariable>::VarType,
        protocol: <PortMappingProtocol as SVariable>::VarType,
        manage: <ArgManage as SVariable>::VarType,
        take: <PortMappingNumberOfEntries as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("GetListOfPortMappings", Self::WANIPCONNECTION_URN)?;

        for argument in &self.in_args {
            match argument.as_str() {
                "NewStartPort" => action.write_argument(argument, start_port),
                "NewEndPort" => action.write_argument(argument, end_port),
                "NewProtocol" => action.write_argument(argument, protocol),
                "NewManage" => action.write_argument(argument, manage),
                "NewNumberOfPorts" => action.write_argument(argument, take),
                _ => anyhow::bail!("Unexpected argument encountered: {}", argument),
            }?
        }

        Ok(action.finish()?)
    }
    pub fn get_list_of_port_mappings_strict(
        &self,
        start_port: <ExternalPort as SVariable>::VarType,
        end_port: <ExternalPort as SVariable>::VarType,
        protocol: <PortMappingProtocol as SVariable>::VarType,
        manage: <ArgManage as SVariable>::VarType,
        take: <PortMappingNumberOfEntries as SVariable>::VarType,
    ) -> anyhow::Result<String> {
        let mut action = WritableAction::new("GetListOfPortMappings", Self::WANIPCONNECTION_URN)?;

        let mut expected = self.in_args.iter().map(|s| s.as_str());
        // Order is important!

        anyhow::ensure!(expected.next() == Some("NewStartPort"));
        action.write_argument("NewStartPort", start_port)?;

        anyhow::ensure!(expected.next() == Some("NewEndPort"));
        action.write_argument("NewEndPort", end_port)?;

        anyhow::ensure!(expected.next() == Some("NewProtocol"));
        action.write_argument("NewProtocol", protocol)?;

        anyhow::ensure!(expected.next() == Some("NewManage"));
        action.write_argument("NewManage", manage)?;

        anyhow::ensure!(expected.next() == Some("NewNumberOfPorts"));
        action.write_argument("NewNumberOfPorts", take)?;

        Ok(action.finish()?)
    }
}

#[derive(Debug)]
pub enum ActionCallError {
    NotSupported,
    HttpError,
    Other(anyhow::Error),
    Action(ActionError),
}

pub type ActionCallResult<T> = Result<T, ActionCallError>;

impl Display for ActionCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionCallError::NotSupported => write!(f, "Action not supported"),
            ActionCallError::HttpError => write!(f, "Http error"),
            ActionCallError::Other(e) => write!(f, "Other: {e}"),
            ActionCallError::Action(action_error) => action_error.fmt(f),
        }
    }
}

impl std::error::Error for ActionCallError {}

impl From<reqwest::Error> for ActionCallError {
    fn from(_value: reqwest::Error) -> Self {
        Self::HttpError
    }
}

impl From<anyhow::Error> for ActionCallError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value)
    }
}

impl From<ActionError> for ActionCallError {
    fn from(value: ActionError) -> Self {
        Self::Action(value)
    }
}

pub trait ScpdService: Send + 'static {
    const URN: URN;
}

#[derive(Debug)]
pub struct ScpdClient<T: ScpdService> {
    pub fetch_client: reqwest::Client,
    pub actions: Vec<Action>,
    pub control_url: String,
    _p: PhantomData<T>,
}

impl<T: ScpdService> ScpdClient<T> {
    pub fn new(scpd: Scpd<'_>, control_url: String) -> Self {
        let actions = scpd
            .actions
            .iter()
            .map(|action| {
                let name = action.name.to_string();
                let mut in_args = Vec::new();
                let mut out_args = Vec::new();
                for arg in &action.arguments {
                    let arg_name = arg.name.to_string();
                    match arg.direction {
                        ArgumentDirection::In => in_args.push(arg_name),
                        ArgumentDirection::Out => out_args.push(arg_name),
                    }
                }
                Action {
                    name,
                    in_args,
                    out_args,
                }
            })
            .collect();

        let fetch_client = reqwest::Client::new();

        Self {
            actions,
            control_url,
            fetch_client,
            _p: PhantomData,
        }
    }

    pub fn action(&self, name: &str) -> Result<&Action, ActionCallError> {
        self.actions
            .iter()
            .find(|a| a.name == name)
            .ok_or(ActionCallError::NotSupported)
    }

    pub async fn run_action<A: ScannableArguments>(
        &self,
        action: &Action,
        payload: String,
    ) -> Result<A, ActionCallError> {
        let header = format!("\"{}#{}\"", T::URN, action.name);
        let request = self
            .fetch_client
            .request(reqwest::Method::POST, self.control_url())
            .header("SOAPAction", header)
            .header(reqwest::header::CONTENT_TYPE, "text/xml")
            .body(payload)
            .build()?;
        let res = self.fetch_client.execute(request).await?;
        tracing::trace!("{} action response status: {}", action.name, res.status());
        let text = res.text().await?;
        let mut reader = quick_xml::Reader::from_str(&text);
        let res = SoapMessage::<Result<ActionResponse<InArgumentPayload>, ActionError>>::read_xml(
            &mut reader,
        )?
        .into_inner()?;
        let mut argument_scanner = ArgumentScanner::new(
            res.args,
            action.out_args.iter().map(AsRef::as_ref).collect(),
        );
        let args = A::scan_arguments(&mut argument_scanner)?;
        Ok(args)
    }

    pub fn control_url(&self) -> &str {
        &self.control_url
    }
}
