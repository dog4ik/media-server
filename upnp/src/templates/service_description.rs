use std::{borrow::Cow, str::FromStr};

use anyhow::Context;
use quick_xml::{
    Writer,
    events::{BytesStart, Event},
};

use crate::{
    FromXml, IntoXml, XmlReaderExt,
    action::{Action, ArgumentDirection},
    service_variables::{DataType, StateVariableDescriptor},
};

use super::SpecVersion;

/// aka SCPD
#[derive(Debug, Clone)]
pub struct ServiceDescription {
    pub spec_version: SpecVersion,
    pub variables: Vec<StateVariableDescriptor>,
    pub actions: Vec<Action>,
}

impl ServiceDescription {
    pub fn into_xml(&self) -> anyhow::Result<Vec<u8>> {
        let mut w = Writer::new(Vec::new());
        let parent = BytesStart::new("scpd");
        w.write_event(Event::Start(parent.to_owned()))?;

        w.write_serializable("specVersion", &self.spec_version)?;

        let action_list = BytesStart::new("actionList");
        w.write_event(Event::Start(action_list.to_owned()))?;
        for action in &self.actions {
            action.write_xml(&mut w)?;
        }
        w.write_event(Event::End(action_list.to_end()))?;

        let service_state_table = BytesStart::new("serviceStateTable");
        w.write_event(Event::Start(service_state_table.to_owned()))?;
        for variable in &self.variables {
            variable.write_xml(&mut w)?;
        }
        w.write_event(Event::End(service_state_table.to_end()))?;

        w.write_event(Event::End(parent.to_end()))?;
        Ok(w.into_inner())
    }
}

#[derive(Debug, Clone)]
pub struct Scpd<'a> {
    pub spec_version: SpecVersion,
    pub variables: Vec<ScpdVariable<'a>>,
    pub actions: Vec<ScpdAction<'a>>,
}

impl<'a> FromXml<'a> for Scpd<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let root = r.read_to_start()?;
        anyhow::ensure!(root.local_name().as_ref() == b"scpd");
        let spec_version = SpecVersion::read_xml(r)?;

        let mut actions = Vec::new();

        let action_list = r.read_to_start()?;
        anyhow::ensure!(action_list.local_name().as_ref() == b"actionList");
        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => {
                    anyhow::ensure!(
                        start.local_name().as_ref() == b"action",
                        "scpd got {:?}",
                        start
                    );
                    actions.push(ScpdAction::read_xml(r)?);
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"actionList");
                    break;
                }
                Event::Text(_) => {}
                r => {
                    Err(anyhow::anyhow!(
                        "expected action or action list end, got {:?}",
                        r
                    ))?;
                }
            }
        }

        let var_list = r.read_to_start()?;
        anyhow::ensure!(var_list.local_name().as_ref() == b"serviceStateTable");
        let mut variables = Vec::new();
        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => {
                    anyhow::ensure!(start.local_name().as_ref() == b"stateVariable");
                    variables.push(ScpdVariable::read_xml(r, start)?);
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"serviceStateTable");
                    break;
                }
                Event::Text(_) => {}
                r => {
                    Err(anyhow::anyhow!(
                        "expected action or action list end, got {:?}",
                        r
                    ))?;
                }
            }
        }

        Ok(Self {
            spec_version,
            variables,
            actions,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScpdAction<'a> {
    pub name: Cow<'a, str>,
    pub arguments: Vec<ScpdActionArgument<'a>>,
}

impl<'a> FromXml<'a> for ScpdAction<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let name = r.read_to_start()?;
        anyhow::ensure!(name.local_name().as_ref() == b"name");
        let name = r.read_text(name.name())?;
        loop {
            let arg_list = r.read_event()?;
            match arg_list {
                Event::Start(start) => {
                    anyhow::ensure!(start.local_name().as_ref() == b"argumentList");
                    break;
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"action");
                    return Ok(Self {
                        name,
                        arguments: Vec::new(),
                    });
                }
                Event::Text(_) => {}
                r => Err(anyhow::anyhow!(
                    "expected end of action or start of arg list, got {:?}",
                    r
                ))?,
            }
        }
        let mut arguments = Vec::new();
        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => {
                    anyhow::ensure!(start.local_name().as_ref() == b"argument");
                    arguments.push(ScpdActionArgument::read_xml(r)?);
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"argumentList");
                    break;
                }
                Event::Text(_) => {}
                r => Err(anyhow::anyhow!(
                    "expected argument or arguments list end, got {:?}",
                    r
                ))?,
            }
        }

        r.read_to_end(quick_xml::name::QName(b"action"))?;

        Ok(Self { name, arguments })
    }
}

#[derive(Debug, Clone)]
pub struct ScpdActionArgument<'a> {
    pub name: Cow<'a, str>,
    pub direction: ArgumentDirection,
    pub related_state_variable: Cow<'a, str>,
}

impl<'a> FromXml<'a> for ScpdActionArgument<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let mut name = None;
        let mut direction = None;
        let mut related_starte_variable = None;
        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => match start.local_name().as_ref() {
                    b"name" => {
                        let text = r.read_text(start.name())?;
                        name = Some(text);
                    }
                    b"direction" => {
                        let text = r.read_text(start.name())?;
                        direction = Some(ArgumentDirection::from_str(&text)?);
                    }
                    b"relatedStateVariable" => {
                        let text = r.read_text(start.name())?;
                        related_starte_variable = Some(text);
                    }
                    _ => {
                        r.read_to_end(start.name())?;
                    }
                },
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"argument");
                    break;
                }
                Event::Text(_) => {}
                r => Err(anyhow::anyhow!(
                    "expected action end or property, got: {:?}",
                    r
                ))?,
            }
        }

        let name = name.context("name")?;
        let direction = direction.context("direction")?;
        let related_state_variable = related_starte_variable.context("related state variable")?;

        Ok(Self {
            name,
            direction,
            related_state_variable,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScpdVariable<'a> {
    pub send_events: bool,
    pub name: Cow<'a, str>,
    pub data_type: DataType,
    pub default_value: Option<Cow<'a, str>>,
    pub allowed_values: Vec<Cow<'a, str>>,
}

impl<'a> ScpdVariable<'a> {
    pub fn read_xml<'b>(
        r: &mut quick_xml::Reader<&'a [u8]>,
        start: BytesStart<'b>,
    ) -> anyhow::Result<Self> {
        let mut name = None;
        let mut data_type = None;
        let mut default_value = None;
        let mut allowed_values = Vec::new();

        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => {
                    match start.local_name().as_ref() {
                        b"name" => {
                            let text = r.read_text(start.name())?;
                            name = Some(text);
                        }
                        b"dataType" => {
                            let text = r.read_text(start.name())?;
                            data_type = Some(DataType::from_str(&text)?);
                        }
                        b"defaultValue" => {
                            let text = r.read_text(start.name())?;
                            default_value = Some(text);
                        }
                        b"allowedValueList" => {
                            while let Ok(event) = r.read_event() {
                                match event {
                                    Event::Start(start) => {
                                        anyhow::ensure!(
                                            start.local_name().as_ref() == b"allowedValue"
                                        );
                                        let text = r.read_text(start.name())?;
                                        allowed_values.push(text);
                                    }
                                    Event::End(end) => {
                                        anyhow::ensure!(
                                            end.local_name().as_ref() == b"allowedValueList"
                                        );
                                        break;
                                    }
                                    Event::Text(_) => {}
                                    r => Err(anyhow::anyhow!(
                                        "expected allowed value or allowed value list end, got {:?}",
                                        r
                                    ))?,
                                }
                            }
                        }
                        _ => {
                            r.read_to_end(start.name())?;
                        }
                    };
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"stateVariable");
                    break;
                }
                Event::Text(_) => {}
                r => Err(anyhow::anyhow!(
                    "expected end of stateVariable, got {:?}",
                    r
                ))?,
            }
        }
        let name = name.context("name")?;
        let data_type = data_type.context("data type")?;

        let send_events = start
            .attributes()
            .flatten()
            .find_map(|a| {
                (a.key.local_name().as_ref() == b"sendEvents").then(|| match a.value.as_ref() {
                    b"no" => false,
                    b"yes" => true,
                    _ => false,
                })
            })
            .unwrap_or(false);

        Ok(Self {
            send_events,
            name,
            data_type,
            default_value,
            allowed_values,
        })
    }
}

impl<'a> FromXml<'a> for ScpdVariable<'a> {
    fn read_xml(r: &mut quick_xml::Reader<&'a [u8]>) -> anyhow::Result<Self> {
        let mut name = None;
        let mut data_type = None;
        let mut default_value = None;
        let mut allowed_values = Vec::new();

        while let Ok(event) = r.read_event() {
            match event {
                Event::Start(start) => {
                    match start.local_name().as_ref() {
                        b"name" => {
                            let text = r.read_text(start.name())?;
                            name = Some(text);
                        }
                        b"dataType" => {
                            let text = r.read_text(start.name())?;
                            data_type = Some(DataType::from_str(&text)?);
                        }
                        b"defaultValue" => {
                            let text = r.read_text(start.name())?;
                            default_value = Some(text);
                        }
                        b"allowedValueList" => {
                            while let Ok(event) = r.read_event() {
                                match event {
                                    Event::Start(start) => {
                                        anyhow::ensure!(
                                            start.local_name().as_ref() == b"allowedValue"
                                        );
                                        let text = r.read_text(start.name())?;
                                        allowed_values.push(text);
                                    }
                                    Event::End(end) => {
                                        anyhow::ensure!(
                                            end.local_name().as_ref() == b"allowedValueList"
                                        );
                                        break;
                                    }
                                    Event::Text(_) => {}
                                    r => Err(anyhow::anyhow!(
                                        "expected allowed value or allowed value list end, got {:?}",
                                        r
                                    ))?,
                                }
                            }
                        }
                        _ => {
                            r.read_to_end(start.name())?;
                        }
                    };
                }
                Event::End(end) => {
                    anyhow::ensure!(end.local_name().as_ref() == b"stateVariable");
                    break;
                }
                Event::Text(_) => {}
                r => Err(anyhow::anyhow!(
                    "expected end of stateVariable, got {:?}",
                    r
                ))?,
            }
        }
        let name = name.context("name")?;
        let data_type = data_type.context("data type")?;

        Ok(Self {
            // we set later it outside where we can access attributes
            send_events: false,
            name,
            data_type,
            default_value,
            allowed_values,
        })
    }
}
