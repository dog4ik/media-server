use core::str;

use anyhow::Context;
use quick_xml::events::{BytesStart, BytesText, Event};
use serde::{Deserialize, Serialize};

use super::{IntoXml, XmlWriter};

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum DataType {
    Ui1,
    Ui2,
    Ui4,
    Ui8,
    I1,
    I2,
    I4,
    I8,
    Int,
    R4,
    R8,
    Number,
    Float,
    Fixed14_4,
    Char,
    #[default]
    String,
    Date,
    DateTime,
    DateTimeTz,
    Time,
    TimeTz,
    Boolean,
    BinBase64,
    BinHex,
    Uri,
    Uuid,
}

fn parse_bool(str_val: &str) -> anyhow::Result<bool> {
    match str_val {
        "1" => Ok(true),
        "0" => Ok(false),
        "true" => Ok(true),
        "false" => Ok(false),
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err(anyhow::anyhow!("Unknown boolean value: {str_val}")),
    }
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            DataType::Ui1 => "ui1",
            DataType::Ui2 => "ui2",
            DataType::Ui4 => "ui4",
            DataType::Ui8 => "ui8",
            DataType::I1 => "i1",
            DataType::I2 => "i2",
            DataType::I4 => "i4",
            DataType::I8 => "i8",
            DataType::Int => "int",
            DataType::R4 => "r4",
            DataType::R8 => "r8",
            DataType::Number => "number",
            DataType::Float => "float",
            DataType::Fixed14_4 => "fixed.14.4",
            DataType::Char => "char",
            DataType::String => "string",
            DataType::Date => "date",
            DataType::DateTime => "dateTime",
            DataType::DateTimeTz => "dateTime.tz",
            DataType::Time => "time",
            DataType::TimeTz => "time.tz",
            DataType::Boolean => "boolean",
            DataType::BinBase64 => "bin.base64",
            DataType::BinHex => "bin.hex",
            DataType::Uri => "uri",
            DataType::Uuid => "uuid",
        };
        write!(f, "{name}")
    }
}

impl std::str::FromStr for DataType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ui1" => Ok(DataType::Ui1),
            "ui2" => Ok(DataType::Ui2),
            "ui4" => Ok(DataType::Ui4),
            "ui8" => Ok(DataType::Ui8),
            "i1" => Ok(DataType::I1),
            "i2" => Ok(DataType::I2),
            "i4" => Ok(DataType::I4),
            "int" => Ok(DataType::Int),
            "r4" => Ok(DataType::R4),
            "r8" => Ok(DataType::R8),
            "number" => Ok(DataType::Number),
            "float" => Ok(DataType::Float),
            "fixed14_4" => Ok(DataType::Fixed14_4),
            "char" => Ok(DataType::Char),
            "string" => Ok(DataType::String),
            "date" => Ok(DataType::Date),
            "dateTime" => Ok(DataType::DateTime),
            "dateTimeTz" => Ok(DataType::DateTimeTz),
            "time" => Ok(DataType::Time),
            "timeTz" => Ok(DataType::TimeTz),
            "boolean" => Ok(DataType::Boolean),
            "bin.base64" => Ok(DataType::BinBase64),
            "bin.hex" => Ok(DataType::BinHex),
            "uri" => Ok(DataType::Uri),
            "uuid" => Ok(DataType::Uuid),
            data_type => Err(anyhow::anyhow!("unrecognized data type: {data_type}")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Range {
    pub start: isize,
    pub end: isize,
    pub step: Option<isize>,
}

impl IntoXml for Range {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        let parent = BytesStart::new("allowedValueRange");
        w.write_event(Event::Start(parent.clone()))?;
        w.create_element("minimum")
            .write_text_content(BytesText::new(&self.start.to_string()))?;
        w.create_element("maximum")
            .write_text_content(BytesText::new(&self.end.to_string()))?;
        if let Some(step) = self.step {
            w.create_element("step")
                .write_text_content(BytesText::new(&step.to_string()))?;
        }
        w.write_event(Event::End(parent.to_end()))
    }
}

/// Convert types into upnpn values
pub trait IntoUpnpValue: IntoXml {
    const TYPE_NAME: DataType = DataType::String;
    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized;
}

impl IntoUpnpValue for u8 {
    const TYPE_NAME: DataType = DataType::Ui1;

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u8")
    }
}

impl IntoXml for u8 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for u16 {
    const TYPE_NAME: DataType = DataType::Ui2;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u8")
    }
}

impl IntoXml for u16 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for u32 {
    const TYPE_NAME: DataType = DataType::Ui4;

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u32")
    }
}

impl IntoXml for u32 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for u64 {
    const TYPE_NAME: DataType = DataType::Ui8;

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u64")
    }
}

impl IntoXml for u64 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for i8 {
    const TYPE_NAME: DataType = DataType::I1;

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u8")
    }
}

impl IntoXml for i8 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for i16 {
    const TYPE_NAME: DataType = DataType::I2;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse i16")
    }
}

impl IntoXml for i16 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for i32 {
    const TYPE_NAME: DataType = DataType::I4;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse i32")
    }
}

impl IntoXml for i32 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for i64 {
    const TYPE_NAME: DataType = DataType::I8;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse i64")
    }
}

impl IntoXml for i64 {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for bool {
    const TYPE_NAME: DataType = DataType::Boolean;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        parse_bool(value)
    }
}

impl IntoXml for bool {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        let val = if *self { "1" } else { "0" };
        w.write_event(Event::Text(BytesText::new(val)))
    }
}

impl IntoUpnpValue for uuid::Uuid {
    const TYPE_NAME: DataType = DataType::Uuid;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse uuid")
    }
}

impl IntoXml for uuid::Uuid {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::from_escaped(self.to_string())))
    }
}

impl IntoUpnpValue for String {
    const TYPE_NAME: DataType = DataType::String;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse string")
    }
}

impl IntoXml for String {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::new(self)))
    }
}

impl IntoUpnpValue for reqwest::Url {
    const TYPE_NAME: DataType = DataType::Uri;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse url")
    }
}

impl IntoXml for reqwest::Url {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        let url = self.to_string();
        w.write_event(Event::Text(BytesText::new(&url)))
    }
}

impl IntoUpnpValue for std::net::Ipv4Addr {
    const TYPE_NAME: DataType = DataType::String;
    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse url")
    }
}

impl IntoXml for std::net::Ipv4Addr {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        let url = self.to_string();
        w.write_event(Event::Text(BytesText::new(&url)))
    }
}

impl IntoXml for &str {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        w.write_event(Event::Text(BytesText::new(self)))
    }
}

impl<T: IntoUpnpValue> IntoUpnpValue for Option<T> {
    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        if value.is_empty() {
            Ok(Self::None)
        } else {
            T::from_xml_value(value).map(Some)
        }
    }
}

impl<T: IntoXml> IntoXml for Option<T> {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        match self {
            Some(v) => v.write_xml(w),
            None => Ok(()),
        }
    }
}

#[derive(Clone)]
pub struct StateVariableDescriptor {
    pub name: &'static str,
    pub kind: DataType,
    pub send_events: bool,
    pub range: Option<Range>,
    pub allowed_list: Option<&'static [&'static str]>,
    pub default: Option<&'static (dyn IntoXml + Send + Sync)>,
}

impl std::fmt::Debug for StateVariableDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("StateVariableDescriptor");
        s.field("name", &self.name);
        s.field("kind", &self.kind);
        s.field("send_events", &self.send_events);
        s.field("range", &self.range);
        s.field("allowed_list", &self.allowed_list);
        let default = self.default.map(|d| d.into_string().unwrap());
        s.field("default", &default);
        s.finish()
    }
}

impl IntoXml for StateVariableDescriptor {
    fn write_xml(&self, w: &mut XmlWriter) -> std::io::Result<()> {
        let send_events = match self.send_events {
            true => "yes",
            false => "no",
        };
        let parent =
            BytesStart::new("stateVariable").with_attributes([("sendEvents", send_events)]);
        w.write_event(Event::Start(parent.clone()))?;
        w.create_element("name")
            .write_text_content(BytesText::new(self.name))?;
        w.create_element("dataType")
            .write_text_content(BytesText::new(&self.kind.to_string()))?;
        if let Some(allowed_list) = self.allowed_list {
            w.create_element("allowedValueList")
                .write_inner_content(|w| {
                    for val in allowed_list {
                        w.create_element("allowedValue")
                            .write_text_content(BytesText::new(val))?;
                    }
                    Ok(())
                })?;
        };
        if let Some(range) = self.range {
            range.write_xml(w)?;
        }
        if let Some(default_value) = self.default {
            w.create_element("defaultValue")
                .write_inner_content(|w| default_value.write_xml(w))?;
        }
        w.write_event(Event::End(parent.to_end()))?;
        Ok(())
    }
}

impl StateVariableDescriptor {
    pub fn from_variable<S: SVariable>() -> Self {
        Self {
            name: S::VAR_NAME,
            kind: S::VarType::TYPE_NAME,
            send_events: S::SEND_EVENTS,
            allowed_list: S::ALLOWED_VALUE_LIST,
            range: S::RANGE,
            default: S::default(),
        }
    }
}

// Playground

pub trait SVariable: Sized {
    type VarType: IntoUpnpValue;

    const VAR_NAME: &str;
    const SEND_EVENTS: bool = false;
    const RANGE: Option<Range> = None;
    const ALLOWED_VALUE_LIST: Option<&[&str]> = None;

    fn default() -> Option<&'static (dyn IntoXml + Send + Sync)> {
        None
    }
}

#[derive(Debug, Clone)]
#[allow(unused)]
struct VolumeVal(String);

impl SVariable for VolumeVal {
    type VarType = String;
    const VAR_NAME: &'static str = "Volume";
    fn default() -> Option<&'static (dyn IntoXml + Send + Sync)> {
        Some(&"0")
    }
}
