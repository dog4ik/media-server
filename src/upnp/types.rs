use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Root {
    #[serde(rename = "@xmlns")]
    pub xmlns: String,
    #[serde(rename = "@xmlns:pnpx")]
    pub xmlns_pnpx: String,
    #[serde(rename = "@xmlns:df")]
    pub xmlns_df: String,
    #[serde(rename = "@xmlns:sec")]
    pub xmlns_sec: String,
    #[serde(rename = "$text")]
    pub text: Option<String>,
    #[serde(rename = "specVersion")]
    pub spec_version: SpecVersion,
    pub device: Device,
}

#[derive(Serialize, Deserialize)]
pub struct SpecVersion {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub major: String,
    pub minor: String,
}

#[derive(Serialize, Deserialize)]
pub struct Device {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    #[serde(rename = "deviceType")]
    pub device_type: String,
    #[serde(rename = "X_compatibleId")]
    pub pnpx_x_compatible_id: String,
    #[serde(rename = "X_deviceCategory")]
    pub df_x_device_category: String,
    #[serde(rename = "X_DLNADOC")]
    pub dlna_x_dlnadoc: DlnaXDlnadoc,
    #[serde(rename = "friendlyName")]
    pub friendly_name: String,
    pub manufacturer: String,
    #[serde(rename = "manufacturerURL")]
    pub manufacturer_url: String,
    #[serde(rename = "modelDescription")]
    pub model_description: String,
    #[serde(rename = "modelName")]
    pub model_name: String,
    #[serde(rename = "modelNumber")]
    pub model_number: String,
    #[serde(rename = "modelURL")]
    pub model_url: String,
    #[serde(rename = "serialNumber")]
    pub serial_number: String,
    #[serde(rename = "UDN")]
    pub udn: String,
    #[serde(rename = "iconList")]
    pub icon_list: IconList,
    #[serde(rename = "serviceList")]
    pub service_list: ServiceList,
    #[serde(rename = "ProductCap")]
    pub sec_product_cap: String,
    #[serde(rename = "X_hardwareId")]
    pub pnpx_x_hardware_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct DlnaXDlnadoc {
    #[serde(rename = "@xmlns:dlna")]
    pub xmlns_dlna: String,
    #[serde(rename = "$text")]
    pub text: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct IconList {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub icon: Vec<Icon>,
}

#[derive(Serialize, Deserialize)]
pub struct Icon {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub mimetype: String,
    pub width: String,
    pub height: String,
    pub depth: String,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
pub struct ServiceList {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    pub service: Vec<Service>,
}

#[derive(Serialize, Deserialize)]
pub struct Service {
    #[serde(rename = "$text")]
    pub text: Option<String>,
    #[serde(rename = "serviceType")]
    pub service_type: String,
    #[serde(rename = "serviceId")]
    pub service_id: String,
    #[serde(rename = "controlURL")]
    pub control_url: String,
    #[serde(rename = "eventSubURL")]
    pub event_sub_url: String,
    #[serde(rename = "SCPDURL")]
    pub scpdurl: String,
}

