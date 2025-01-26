use std::sync::Arc;

use crate::{
    action::{ActionError, ActionErrorCode, InArgumentPayload, IntoValueList},
    service_variables::{IntoUpnpValue, SVariable},
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
    //fn event_handler<'a>(
    //    &self,
    //    name: &'a str,
    //    inputs: ArgumentScanner<'a>,
    //) -> impl std::future::Future<Output = anyhow::Result<impl IntoValueList>> + Send;
}

#[derive(Debug, Clone)]
pub struct ArgumentScanner<'a> {
    payload: std::vec::IntoIter<InArgumentPayload<'a>>,
    // TODO: make it generic
    expected: std::vec::IntoIter<&'a str>,
}

impl<'a> ArgumentScanner<'a> {
    pub fn new(payload: Vec<InArgumentPayload<'a>>, expected: Vec<&'a str>) -> Self {
        Self {
            payload: payload.into_iter(),
            expected: expected.into_iter(),
        }
    }

    pub fn next<T: IntoUpnpValue>(&mut self) -> Result<T, ActionError> {
        let Some((expected_next, next)) = self.expected.next().zip(self.payload.next()) else {
            return Err(ActionErrorCode::InvalidArguments.into());
        };
        if next.name() != expected_next {
            return Err(ActionErrorCode::InvalidArguments.into());
        }
        let Ok(arg) = T::from_xml_value(&next.value) else {
            return Err(ActionErrorCode::InvalidArguments.into());
        };
        Ok(arg)
    }

    pub fn next_unchecked<T: IntoUpnpValue>(&mut self) -> Result<T, ActionError> {
        let _ = self.expected.next();
        let Some(next) = self.payload.next() else {
            return Err(ActionErrorCode::InvalidArguments.into());
        };
        let Ok(arg) = T::from_xml_value(&next.value) else {
            return Err(ActionErrorCode::InvalidArguments.into());
        };
        Ok(arg)
    }

    pub fn next_var<T: SVariable>(&mut self) -> Result<T::VarType, ActionError> {
        self.next::<T::VarType>()
    }

    pub fn next_var_unchecked<T: SVariable>(&mut self) -> Result<T::VarType, ActionError> {
        self.next_unchecked::<T::VarType>()
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
}
