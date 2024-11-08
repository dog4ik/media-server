use core::str;
use std::{borrow::Cow, collections::HashMap, fmt::Display};

use axum::{http::HeaderMap, response::IntoResponse};
use axum_extra::headers::{self, HeaderMapExt};
use quick_xml::events::{BytesDecl, BytesStart, BytesText, Event};
use reqwest::StatusCode;

use crate::{service::ArgumentScanner, XmlReaderExt};

use super::{
    service_variables::{IntoUpnpValue, SVariable, StateVariableDescriptor},
    urn::URN,
    FromXml, IntoXml, XmlWriter,
};

#[derive(Debug, Clone)]
pub struct Argument {
    name: &'static str,
    related_variable: StateVariableDescriptor,
}

#[derive(Debug, Clone, Copy)]
pub enum ArgumentDirection {
    In,
    Out,
}

impl From<ArgumentDirection> for &str {
    fn from(value: ArgumentDirection) -> Self {
        match value {
            ArgumentDirection::In => "in",
            ArgumentDirection::Out => "out",
        }
    }
}

impl Argument {
    fn into_sv<S: SVariable>(name: &'static str) -> Self {
        let state_variable = StateVariableDescriptor {
            name: S::VAR_NAME,
            kind: S::VarType::TYPE_NAME,
            send_events: S::SEND_EVENTS,
            range: S::RANGE,
            default: S::default(),
            allowed_list: S::ALLOWED_VALUE_LIST,
        };
        Self {
            name,
            related_variable: state_variable,
        }
    }

    pub fn write_xml<T: std::io::Write>(
        &self,
        w: &mut quick_xml::Writer<T>,
        direction: ArgumentDirection,
    ) -> quick_xml::Result<()> {
        let parent = BytesStart::new("argument");
        w.write_event(Event::Start(parent.clone()))?;
        w.create_element("name")
            .write_text_content(BytesText::new(&self.name))?;

        w.create_element("direction")
            .write_text_content(BytesText::new(direction.into()))?;

        w.create_element("relatedStateVariable")
            .write_text_content(BytesText::new(&self.related_variable.name))?;
        w.write_event(Event::End(parent.to_end()))?;
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug)]
pub struct OutArgument<T: SVariable> {
    pub name: String,
    pub var: T::VarType,
}

impl<T: SVariable> OutArgument<T> {
    pub fn into_inner(self) -> T::VarType {
        self.var
    }
}

impl<T: SVariable> AsRef<T::VarType> for OutArgument<T> {
    fn as_ref(&self) -> &T::VarType {
        &self.var
    }
}

impl<T: SVariable> AsMut<T::VarType> for OutArgument<T> {
    fn as_mut(&mut self) -> &mut T::VarType {
        &mut self.var
    }
}

impl<A: SVariable> Display for OutArgument<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.var.into_string().unwrap();
        write!(
            f,
            "{}: {value} @ State variable: {}",
            self.name,
            A::VAR_NAME
        )
    }
}

#[derive(Debug, Clone)]
pub struct Action {
    action_name: String,
    in_variables: Vec<Argument>,
    out_variables: Vec<Argument>,
}

impl Action {
    pub fn empty(name: &str) -> Self {
        Self {
            action_name: name.to_string(),
            in_variables: Vec::new(),
            out_variables: Vec::new(),
        }
    }

    pub fn add_input<T: SVariable>(&mut self, name: &'static str) {
        self.in_variables.push(Argument::into_sv::<T>(name));
    }

    pub fn add_output<T: SVariable>(&mut self, name: &'static str) {
        self.out_variables.push(Argument::into_sv::<T>(name));
    }

    pub fn name(&self) -> &str {
        &self.action_name
    }

    pub fn in_variables(&self) -> &Vec<Argument> {
        &self.in_variables
    }

    pub fn out_variables(&self) -> &Vec<Argument> {
        &self.out_variables
    }

    pub fn input_scanner<'a>(&'a self, input: Vec<InArgumentPayload<'a>>) -> ArgumentScanner<'a> {
        ArgumentScanner::new(input, self.in_variables())
    }

    pub fn map_out_variables(&self, list: Vec<Box<dyn IntoXml>>) -> Vec<OutArgumentsPayload> {
        let out_variables = self.out_variables();
        let mut arguments = Vec::with_capacity(out_variables.len());
        for (arg, val) in self.out_variables.iter().zip(list.into_iter()) {
            arguments.push(OutArgumentsPayload {
                name: arg.name().to_owned(),
                value: val,
            });
        }
        if arguments.len() != out_variables.len() {
            tracing::warn!(
                "Mismatched output arguments length from {} action ({}/{})",
                self.name(),
                arguments.len(),
                out_variables.len(),
            );
        }
        arguments
    }
}

impl IntoXml for Action {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let parent = BytesStart::new("action");
        w.write_event(Event::Start(parent.clone()))?;

        w.create_element("name")
            .write_text_content(BytesText::new(&self.action_name))?;

        let argument_list = BytesStart::new("argumentList");
        w.write_event(Event::Start(argument_list.clone()))?;
        for argument in &self.in_variables {
            argument.write_xml(w, ArgumentDirection::In)?;
        }
        for argument in &self.out_variables {
            argument.write_xml(w, ArgumentDirection::Out)?;
        }
        w.write_event(Event::End(argument_list.to_end()))?;

        w.write_event(Event::End(parent.to_end()))?;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct SoapMessage<T> {
    inner: T,
}

impl<T> SoapMessage<T> {
    pub fn new(payload: T) -> Self {
        Self { inner: payload }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<'a, T: FromXml<'a>> SoapMessage<T> {
    pub fn from_xml(raw_xml: &'a [u8]) -> anyhow::Result<Self> {
        use quick_xml::Reader;
        let mut r = Reader::from_reader(raw_xml);

        let envelope = r.read_to_start()?.into_owned();
        anyhow::ensure!(envelope.local_name().as_ref() == b"Envelope");
        let body = r.read_to_start()?.into_owned();
        anyhow::ensure!(body.local_name().as_ref() == b"Body");

        let payload = T::read_xml(&mut r)?;

        r.read_to_end(body.name())?;
        r.read_to_end(envelope.name())?;
        Ok(Self { inner: payload })
    }
}

impl<T: IntoXml> SoapMessage<T> {
    pub fn into_xml(self) -> anyhow::Result<String> {
        use quick_xml::Writer;
        let mut w = Writer::new(Vec::new());
        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        let envelope = BytesStart::new("s:Envelope").with_attributes([
            ("xmlns:s", "http://schemas.xmlsoap.org/soap/envelope/"),
            (
                "s:encodingStyle",
                "http://schemas.xmlsoap.org/soap/encoding/",
            ),
        ]);
        let envelope_end = envelope.to_end().into_owned();
        w.write_event(Event::Start(envelope.clone()))?;

        let body = BytesStart::new("s:Body");
        let body_end = body.to_end().into_owned();
        w.write_event(Event::Start(body))?;

        self.inner.write_xml(&mut w)?;

        w.write_event(Event::End(body_end))?;
        w.write_event(Event::End(envelope_end))?;
        Ok(String::from_utf8(w.into_inner())?)
    }
}

impl<T: IntoXml> IntoResponse for SoapMessage<T> {
    fn into_response(self) -> axum::response::Response {
        let mut header_map = HeaderMap::new();
        header_map.typed_insert(headers::ContentType::xml());
        let body = self.into_xml().expect("serialization not fail");
        (header_map, body).into_response()
    }
}

/// An SCPD action inside Soap message.
/// The action consists of its name used in the services
#[derive(Debug, Clone)]
pub struct ActionPayload<'a> {
    pub name: String,
    pub arguments: Vec<InArgumentPayload<'a>>,
}

impl<'a> FromXml<'a> for ActionPayload<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let action_name_tag = r.read_to_start()?;
        let action_name_tag_end = action_name_tag.to_end().into_owned();
        let action_name = String::from_utf8(action_name_tag.local_name().into_inner().to_vec())?;
        let mut arguments = Vec::new();

        loop {
            let next = r.read_event_err_eof()?.into_owned();
            match next {
                Event::Start(var) => {
                    let name = String::from_utf8(var.local_name().into_inner().to_vec())?;
                    let value = r.read_text(var.name())?;
                    arguments.push(InArgumentPayload { name, value });
                }
                Event::End(end) if end == action_name_tag_end => {
                    break;
                }
                _ => (),
            }
        }

        Ok(Self {
            name: action_name,
            arguments,
        })
    }
}

impl IntoXml for ActionPayload<'_> {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let action = BytesStart::new(self.name());
        let action_end = action.to_end().into_owned();
        w.write_event(Event::Start(action))?;

        for argument in &self.arguments {
            w.create_element(argument.name())
                .write_text_content(BytesText::new(&argument.value))?;
        }

        w.write_event(Event::End(action_end))
    }
}

impl ActionPayload<'_> {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn arguments_map(&self) -> HashMap<String, &str> {
        self.arguments
            .iter()
            .map(|a| (a.name.clone(), a.value.as_ref()))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct InArgumentPayload<'a> {
    pub name: String,
    pub value: Cow<'a, str>,
}

impl InArgumentPayload<'_> {
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug)]
pub struct OutArgumentsPayload {
    pub name: String,
    pub value: Box<dyn IntoXml>,
}

impl OutArgumentsPayload {
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug)]
pub struct ActionResponse {
    pub action_name: String,
    pub service_urn: URN,
    pub args: Vec<OutArgumentsPayload>,
}

impl IntoXml for ActionResponse {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let action = BytesStart::new(format!("u:{}Response", self.action_name))
            .with_attributes([("xmlns:u", self.service_urn.to_string().as_str())]);
        let action_end = action.to_end().into_owned();
        w.write_event(Event::Start(action))?;

        for argument in &self.args {
            w.create_element(argument.name())
                .write_inner_content(|w| argument.value.write_xml(w))?;
        }

        w.write_event(Event::End(action_end))
    }
}

impl IntoResponse for ActionResponse {
    fn into_response(self) -> axum::response::Response {
        SoapMessage::new(self).into_response()
    }
}

pub trait IntoValueList {
    fn into_value_list(self) -> Vec<Box<dyn IntoXml>>;
}

impl<T: IntoUpnpValue + 'static> IntoValueList for T {
    fn into_value_list(self) -> Vec<Box<dyn IntoXml>> {
        vec![Box::new(self)]
    }
}

impl IntoValueList for () {
    fn into_value_list(self) -> Vec<Box<dyn IntoXml>> {
        vec![]
    }
}

impl IntoValueList for Vec<Box<dyn IntoXml>> {
    fn into_value_list(self) -> Vec<Box<dyn IntoXml>> {
        self
    }
}

macro_rules! impl_tuples_into_value_list {
    () => {};

    ($(($($types:ident),*)),*) => {
        $(
            #[allow(non_snake_case, unused_variables)]
            impl<$($types: IntoUpnpValue + 'static),*> IntoValueList for ($($types,)*) {
                fn into_value_list(self) -> Vec<Box<dyn IntoXml>> {
                    let ($($types,)*) = self;
                    let mut args: Vec<Box<dyn IntoXml>> = Vec::new();
                    $(
                        args.push(Box::new($types));
                    )*
                    args
                }
            }
        )*
    };
}

impl_tuples_into_value_list! {
    (A),
    (A, B),
    (A, B, C),
    (A, B, C, D),
    (A, B, C, D, E),
    (A, B, C, D, E, F),
    (A, B, C, D, E, F, G),
    (A, B, C, D, E, F, G, H),
    (A, B, C, D, E, F, G, H, I),
    (A, B, C, D, E, F, G, H, I, J),
    (A, B, C, D, E, F, G, H, I, J, K),
    (A, B, C, D, E, F, G, H, I, J, K, L)
}

#[derive(Debug, Clone, Copy)]
pub enum ActionErrorCode {
    /// No action by that name at this service.
    InvalidAction,
    /// Could be any of the following: not enough in args, args in the wrong
    /// order, one or more in args are of the wrong data type.
    InvalidArguments,
    /// Is allowed to be returned if current state of service prevents invoking
    /// that action
    ActionFailed,
    /// The argument value is invalid
    ArgumentInvalid,
    /// An argument value is less than the minimum or more than the
    /// maximum value of the allowed value range, or is not in the allowed
    /// value list
    ArgumentValueOutOfRange,
    /// Optional Action Not
    /// Implemented
    OptionalActionNotImplemented,
    /// The device does not have sufficient memory available to complete the
    /// action.
    OutOfMemory,
    /// The device has encountered an error condition which it cannot resolve
    /// itself and required human intervention such as a reset or power cycle.
    HumanInterventionRequired,
    /// A string argument is too long for the device to handle properly.
    StringArgumentTooLong,
    Other(u16),
}

impl ActionErrorCode {
    pub fn code(&self) -> u16 {
        match self {
            ActionErrorCode::InvalidAction => 401,
            ActionErrorCode::InvalidArguments => 402,
            ActionErrorCode::ActionFailed => 501,
            ActionErrorCode::ArgumentInvalid => 600,
            ActionErrorCode::ArgumentValueOutOfRange => 601,
            ActionErrorCode::OptionalActionNotImplemented => 602,
            ActionErrorCode::OutOfMemory => 603,
            ActionErrorCode::HumanInterventionRequired => 604,
            ActionErrorCode::StringArgumentTooLong => 605,
            ActionErrorCode::Other(code) => *code,
        }
    }
}

impl From<ActionErrorCode> for ActionError {
    fn from(code: ActionErrorCode) -> Self {
        Self {
            code,
            description: None,
        }
    }
}

#[derive(Debug)]
pub struct ActionError {
    pub code: ActionErrorCode,
    pub description: Option<String>,
}

impl ActionError {
    pub fn not_implemented() -> Self {
        Self {
            code: ActionErrorCode::OptionalActionNotImplemented,
            description: None,
        }
    }
}

impl From<anyhow::Error> for ActionError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            code: ActionErrorCode::ActionFailed,
            description: Some(err.to_string()),
        }
    }
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(description) = &self.description {
            write!(f, "{}: {}", self.code.code(), description)
        } else {
            write!(f, "{}", self.code.code())
        }
    }
}
impl std::error::Error for ActionError {}

impl IntoXml for ActionError {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let parent = BytesStart::new("s:Fault");
        let parent_end = parent.to_end().into_owned();
        w.write_event(Event::Start(parent.clone()))?;

        w.create_element("faultcode")
            .write_text_content(BytesText::new("s:Client"))?;
        w.create_element("faultstring")
            .write_text_content(BytesText::new("UPnPError"))?;
        let detail = BytesStart::new("detail");
        let detail_end = detail.to_end().into_owned();
        w.write_event(Event::Start(detail.clone()))?;

        w.create_element("UPnPError")
            .with_attribute(("xmlns", "schemas-upnp-org:control-1-0"))
            .write_inner_content::<_, quick_xml::Error>(|w| {
                w.create_element("errorCode")
                    .write_text_content(BytesText::new(&self.code.code().to_string()))?;
                if let Some(description) = &self.description {
                    w.create_element("errorDescription")
                        .write_text_content(BytesText::new(description))?;
                }
                Ok(())
            })?;

        w.write_event(Event::End(detail_end))?;
        w.write_event(Event::End(parent_end))
    }
}

impl IntoResponse for ActionError {
    fn into_response(self) -> axum::response::Response {
        let status_code = StatusCode::INTERNAL_SERVER_ERROR;
        let body = SoapMessage::new(self);
        (status_code, body).into_response()
    }
}

#[cfg(test)]
mod tests {

    use crate::action::SoapMessage;

    use super::ActionPayload;

    #[test]
    fn parse_action_payload_xml() {
        let raw = br#"<?xml version="1.0"?>
<s:Envelope
xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
<u:actionName xmlns:u="urn:schemas-upnp-org:service:serviceType:v">
<argumentName>in arg value</argumentName>
<anotherArgument>another value</anotherArgument>
<!-- other in args and their values go here, if any -->
</u:actionName>
</s:Body>
</s:Envelope>"#;
        let payload: SoapMessage<ActionPayload> = SoapMessage::from_xml(raw).unwrap();
        let payload = payload.into_inner();
        assert_eq!(payload.name, "actionName");
        let args = payload.arguments_map();
        assert_eq!(args.get("argumentName"), Some("in arg value").as_ref());
        assert_eq!(args.get("anotherArgument"), Some("another value").as_ref());
    }
}
