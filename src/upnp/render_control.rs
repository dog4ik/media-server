use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Scpd {
    #[serde(rename = "@xmlns")]
    pub xmlns: String,
    #[serde(rename = "$text")]
    pub text: Option<String>,
    #[serde(rename = "specVersion")]
    pub spec_version: SpecVersion,
    #[serde(rename = "actionList")]
    pub action_list: ActionList,
    #[serde(rename = "serviceStateTable")]
    pub service_state_table: ServiceStateTable,
}

#[derive(Serialize, Deserialize)]
pub struct SpecVersion {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub major: String,
    pub minor: String,
}

#[derive(Serialize, Deserialize)]
pub struct ActionList {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub action: Vec<Action>,
}

#[derive(Serialize, Deserialize)]
pub struct Action {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub name: String,
    #[serde(rename = "argumentList")]
    pub argument_list: ArgumentList,
}

#[derive(Serialize, Deserialize)]
pub struct ArgumentList {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub argument: Vec<Argument>,
}

#[derive(Serialize, Deserialize)]
pub struct Argument {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub name: String,
    pub direction: String,
    #[serde(rename = "relatedStateVariable")]
    pub related_state_variable: String,
}

#[derive(Serialize, Deserialize)]
pub struct ServiceStateTable {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    #[serde(rename = "stateVariable")]
    pub state_variable: Vec<StateVariable>,
}

#[derive(Serialize, Deserialize)]
pub struct StateVariable {
    #[serde(rename = "@sendEvents")]
    pub send_events: String,
    #[serde(rename = "$text")]
    pub text: Option<String>,
    #[serde(rename = "allowedValueRange")]
    pub allowed_value_range: Option<AllowedValueRange>,
    #[serde(rename = "defaultValue")]
    pub default_value: Option<String>,
    #[serde(rename = "allowedValueList")]
    pub allowed_value_list: Option<AllowedValueList>,
    pub name: String,
    #[serde(rename = "dataType")]
    pub data_type: String,
}

#[derive(Serialize, Deserialize)]
pub struct AllowedValueRange {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub minimum: String,
    pub maximum: String,
    pub step: String,
}

#[derive(Serialize, Deserialize)]
pub struct AllowedValueList {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    #[serde(rename = "allowedValue")]
    pub allowed_value: Vec<String>,
}


#[derive(Debug, Clone)]
pub struct RenderControlService {
    url: String,
}

