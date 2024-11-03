use core::str;

use anyhow::Context;
use quick_xml::events::{BytesStart, BytesText, Event};
use serde::{Deserialize, Serialize};

use super::{IntoXml, XmlWriter};

pub enum VariableKind {
    Known(DataType),
}

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

#[derive(Debug, Clone)]
pub enum Value {
    Ui1(u8),
    Ui2(u16),
    Ui4(u32),
    Ui8(u64),
    I1(i8),
    I2(i16),
    I4(i32),
    I8(i64),
    Int(f32),
    R4(f32),
    R8(f64),
    Number(f64),
    Float(f32),
    Fixed14_4(f64),
    Char(char),
    String(String),
    Date(time::OffsetDateTime),
    DateTime(time::OffsetDateTime),
    DateTimeTz(time::OffsetDateTime),
    Time(time::Time),
    TimeTz(time::Time),
    Boolean(bool),
    BinBase64(String),
    BinHex(String),
    Uri(reqwest::Url),
    Uuid(uuid::Uuid),
}

const DATE_FORMAT: time::format_description::well_known::Iso8601 =
    time::format_description::well_known::Iso8601;

impl Value {
    fn from_data_type(data_type: DataType, val: &[u8]) -> anyhow::Result<Self> {
        fn parse_date(str_val: &str) -> anyhow::Result<time::OffsetDateTime> {
            let date = time::OffsetDateTime::parse(str_val, &DATE_FORMAT)?;
            Ok(date)
        }
        fn parse_time(str_val: &str) -> anyhow::Result<time::Time> {
            let date = time::Time::parse(str_val, &DATE_FORMAT)?;
            Ok(date)
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
        let str_val = str::from_utf8(val)?;
        let data = match data_type {
            DataType::Ui1 => Value::Ui1(str_val.parse().context("parse Ui1")?),
            DataType::Ui2 => Value::Ui2(str_val.parse().context("parse Ui2")?),
            DataType::Ui4 => Value::Ui4(str_val.parse().context("parse Ui4")?),
            DataType::Ui8 => Value::Ui8(str_val.parse().context("parse Ui8")?),
            DataType::I1 => Value::I1(str_val.parse().context("parse I1")?),
            DataType::I2 => Value::I2(str_val.parse().context("parse I2")?),
            DataType::I4 => Value::I4(str_val.parse().context("parse I4")?),
            DataType::I8 => Value::I8(str_val.parse().context("parse I8")?),
            DataType::Int => Value::Int(str_val.parse().context("parse Int")?),
            DataType::R4 => Value::R4(str_val.parse().context("parse R4")?),
            DataType::R8 => Value::R8(str_val.parse().context("parse R8")?),
            DataType::Number => Value::Number(str_val.parse().context("parse Number")?),
            DataType::Float => Value::Float(str_val.parse().context("parse Float")?),
            DataType::Fixed14_4 => Value::Fixed14_4(str_val.parse().context("parse Fixed14_4")?),
            DataType::Char => Value::Char(str_val.parse().context("parse Char")?),
            DataType::String => Value::String(str_val.parse().context("parse String")?),
            DataType::Date => Value::Date(parse_date(str_val).context("parse Date")?),
            DataType::DateTime => Value::DateTime(parse_date(str_val).context("parse DateTime")?),
            DataType::DateTimeTz => {
                Value::DateTimeTz(parse_date(str_val).context("parse DateTimeTz")?)
            }
            DataType::Time => Value::Time(parse_time(str_val).context("parse Time")?),
            DataType::TimeTz => Value::TimeTz(parse_time(str_val).context("parse TimeTz")?),
            DataType::Boolean => Value::Boolean(parse_bool(str_val)?),
            DataType::Uri => Value::Uri(str_val.parse().context("parse Uri")?),
            DataType::Uuid => Value::Uuid(str_val.parse().context("parse Uuid")?),
            DataType::BinHex => Value::BinHex(str_val.to_string()),
            DataType::BinBase64 => Value::BinBase64(str_val.to_string()),
        };
        Ok(data)
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let val = match self {
            Value::Ui1(v) => v.to_string(),
            Value::Ui2(v) => v.to_string(),
            Value::Ui4(v) => v.to_string(),
            Value::Ui8(v) => v.to_string(),
            Value::I1(v) => v.to_string(),
            Value::I2(v) => v.to_string(),
            Value::I4(v) => v.to_string(),
            Value::I8(v) => v.to_string(),
            Value::Int(v) => v.to_string(),
            Value::R4(v) => v.to_string(),
            Value::R8(v) => v.to_string(),
            Value::Number(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Fixed14_4(v) => v.to_string(),
            Value::Char(v) => v.to_string(),
            Value::String(v) => v.to_string(),
            Value::Date(v) => v.to_string(),
            Value::DateTime(v) => v.to_string(),
            Value::DateTimeTz(v) => v.to_string(),
            Value::Time(v) => v.to_string(),
            Value::TimeTz(v) => v.to_string(),
            Value::Boolean(v) => {
                if *v {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Value::BinBase64(v) => v.to_string(),
            Value::BinHex(v) => v.to_string(),
            Value::Uri(v) => v.to_string(),
            Value::Uuid(v) => v.to_string(),
        };
        write!(f, "{}", val)
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
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
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
pub trait IntoUpnpValue {
    const TYPE_NAME: DataType = DataType::String;
    fn into_value(&self) -> Value;
    fn from_xml_value(value: &str) -> anyhow::Result<Self>
    where
        Self: Sized;
}

impl IntoUpnpValue for u8 {
    const TYPE_NAME: DataType = DataType::Ui1;
    fn into_value(&self) -> Value {
        Value::Ui1(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u8")
    }
}

impl IntoXml for u8 {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
        let str = self.to_string();
        w.write_event(Event::Text(BytesText::new(&str)))
    }
}

impl IntoUpnpValue for u16 {
    const TYPE_NAME: DataType = DataType::Ui2;
    fn into_value(&self) -> Value {
        Value::Ui2(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u8")
    }
}

impl IntoUpnpValue for u32 {
    const TYPE_NAME: DataType = DataType::Ui4;
    fn into_value(&self) -> Value {
        Value::Ui4(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u32")
    }
}

impl IntoUpnpValue for u64 {
    const TYPE_NAME: DataType = DataType::Ui8;
    fn into_value(&self) -> Value {
        Value::Ui8(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u64")
    }
}

impl IntoUpnpValue for i8 {
    const TYPE_NAME: DataType = DataType::I1;
    fn into_value(&self) -> Value {
        Value::I1(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse u8")
    }
}

impl IntoUpnpValue for i16 {
    const TYPE_NAME: DataType = DataType::I2;
    fn into_value(&self) -> Value {
        Value::I2(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse i16")
    }
}

impl IntoUpnpValue for i32 {
    const TYPE_NAME: DataType = DataType::I4;
    fn into_value(&self) -> Value {
        Value::I4(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse i32")
    }
}

impl IntoUpnpValue for i64 {
    const TYPE_NAME: DataType = DataType::I8;
    fn into_value(&self) -> Value {
        Value::I8(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse i64")
    }
}

impl IntoUpnpValue for bool {
    const TYPE_NAME: DataType = DataType::Boolean;
    fn into_value(&self) -> Value {
        Value::Boolean(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse boolean")
    }
}

impl IntoUpnpValue for uuid::Uuid {
    const TYPE_NAME: DataType = DataType::Uuid;
    fn into_value(&self) -> Value {
        Value::Uuid(*self)
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse uuid")
    }
}

impl IntoUpnpValue for String {
    const TYPE_NAME: DataType = DataType::String;
    fn into_value(&self) -> Value {
        Value::String(self.to_owned())
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse string")
    }
}

impl IntoUpnpValue for reqwest::Url {
    const TYPE_NAME: DataType = DataType::Uri;
    fn into_value(&self) -> Value {
        Value::Uri(self.to_owned())
    }

    fn from_xml_value(value: &str) -> anyhow::Result<Self> {
        value.parse().context("parse url")
    }
}

#[derive(Debug, Clone)]
pub struct StateVariableDescriptor {
    pub name: &'static str,
    pub kind: DataType,
    pub send_events: bool,
    pub range: Option<Range>,
    pub allowed_list: Option<&'static [&'static str]>,
    pub default: Option<String>,
}

impl IntoXml for StateVariableDescriptor {
    fn write_xml(&self, w: &mut XmlWriter) -> quick_xml::Result<()> {
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
                .write_inner_content::<_, quick_xml::Error>(|w| {
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
        if let Some(default_value) = &self.default {
            w.create_element("defaultValue")
                .write_text_content(BytesText::new(default_value))?;
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
            default: S::default().map(|d| d.into_value().to_string()),
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

    fn default() -> Option<Self::VarType> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct VolumeVal(String);

impl SVariable for VolumeVal {
    type VarType = String;
    const VAR_NAME: &'static str = "Volume";
    fn default() -> Option<Self::VarType> {
        Some("0".into())
    }
}
