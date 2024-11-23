use std::{fmt::Display, str::FromStr};

use anyhow::Context;
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

impl FromStr for SpecVersion {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (major, minor) = s.split_once('/').context("split version")?;
        Ok(Self {
            major: major.parse().context("parse major version")?,
            minor: minor.parse().context("parse minor version")?,
        })
    }
}

impl Display for SpecVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UpnpAgent<'a> {
    pub os: &'a str,
    pub os_version: &'a str,
    pub upnp_version: SpecVersion,
    pub product: &'a str,
    pub product_version: &'a str,
}

impl<'a> UpnpAgent<'a> {
    pub fn new(
        (os, os_version): (&'a str, &'a str),
        spec: SpecVersion,
        (product, product_version): (&'a str, &'a str),
    ) -> Self {
        Self {
            os,
            os_version,
            upnp_version: spec,
            product,
            product_version,
        }
    }
}

impl<'a> TryFrom<&'a str> for UpnpAgent<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        let mut split = value.split_ascii_whitespace();
        let os_part = split.next().context("os part")?;
        let upnp_version = split.next().context("upnp version")?;
        let product_part = split.next().context("product part")?;
        let (os, os_version) = os_part.split_once('/').context("split os part")?;
        let (upnp, upnp_version) = upnp_version.split_once('/').context("split upnp version")?;
        anyhow::ensure!(upnp == "UPnP");
        let (product, product_version) = product_part.split_once('/').context("split product")?;
        Ok(Self {
            os,
            os_version,
            upnp_version: upnp_version.parse()?,
            product,
            product_version,
        })
    }
}

impl Display for UpnpAgent<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{os}/{os_version} UPnP/{upnp_version} {product}/{product_version}",
            os = self.os,
            os_version = self.os_version,
            upnp_version = self.upnp_version,
            product = self.product,
            product_version = self.product_version
        )
    }
}
