use quick_xml::{
    events::{BytesStart, Event},
    Writer,
};

use crate::upnp::{action::Action, service_variables::StateVariableDescriptor, IntoXml};

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
