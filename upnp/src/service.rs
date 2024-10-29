use std::sync::Arc;

use crate::{
    action::{ActionError, ActionErrorCode, Argument, ArgumentPayload, IntoValueList},
    service_variables::IntoUpnpValue,
};

use super::{action::Action, templates::service_description::ServiceDescription, urn::URN};

pub trait Service {
    const NAME: &str;
    const URN: URN;

    fn service_description() -> ServiceDescription;
    fn actions() -> Vec<Action>;
    fn control_handler<'a>(
        &self,
        name: &'a str,
        inputs: ArgumentScanner<'a>,
    ) -> impl std::future::Future<Output = anyhow::Result<impl IntoValueList>> + Send;
}

#[derive(Debug, Clone)]
pub struct ArgumentScanner<'a> {
    payload: std::vec::IntoIter<ArgumentPayload>,
    expected: std::slice::Iter<'a, Argument>,
}

impl<'a> ArgumentScanner<'a> {
    pub fn new(payload: Vec<ArgumentPayload>, expected: &'a Vec<Argument>) -> Self {
        Self {
            payload: payload.into_iter(),
            expected: expected.iter(),
        }
    }

    pub fn next<T: IntoUpnpValue>(&mut self) -> Result<T, ActionError> {
        let Some(expected_next) = self.expected.next() else {
            return Err(ActionErrorCode::InvalidArguments.into());
        };
        let Some(next) = self.payload.next() else {
            return Err(ActionErrorCode::InvalidArguments.into());
        };
        if next.name() != expected_next.name() {
            return Err(ActionErrorCode::InvalidArguments.into());
        }
        let Ok(arg) = T::from_xml_value(&next.value) else {
            return Err(ActionErrorCode::InvalidArguments.into());
        };
        Ok(arg)
    }
}

#[derive(Debug, Clone)]
pub struct UpnpService<S: Service> {
    pub actions: Arc<Vec<Action>>,
    pub s: S,
}

impl<S: Service> UpnpService<S> {
    pub fn new(service: S) -> Self {
        let actions = Arc::new(S::actions());
        Self {
            actions,
            s: service,
        }
    }

    pub fn find_action(&self, name: &str) -> Result<&Action, ActionError> {
        Ok(self
            .actions
            .iter()
            .find(|a| a.name() == name)
            .ok_or(ActionErrorCode::InvalidAction)?)
    }

    pub fn input_scanner<'a>(
        &'a self,
        name: &str,
        input: Vec<ArgumentPayload>,
    ) -> Result<ArgumentScanner<'a>, ActionError> {
        let action = self.find_action(name)?;
        Ok(ArgumentScanner::new(input, action.in_variables()))
    }
}
