use serde::{Deserialize, Serialize};

pub mod service_description;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecVersion {
    pub major: usize,
    pub minor: usize,
}

impl SpecVersion {
    /// UPnP2.0 spec version
    pub const fn upnp_v2() -> Self {
        Self { major: 2, minor: 0 }
    }
    pub const fn upnp_v1_1() -> Self {
        Self { major: 1, minor: 1 }
    }
    pub const fn upnp_v1() -> Self {
        Self { major: 1, minor: 0 }
    }
}
