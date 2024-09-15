use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

pub enum VariableKind {
    Known(DataType),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum DataType {
    Ui1,
    Ui2,
    Ui4,
    Ui8,
    I1,
    I2,
    I4,
    Int,
    R4,
    R8,
    Number,
    Float,
    Fixed14_4,
    Char,
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
    Date(time::Date),
    DateTime(time::OffsetDateTime),
    DateTimeTz(time::OffsetDateTime),
    Time(time::Time),
    TimeTz(time::Time),
    Boolean(bool),
    BinBase64(Vec<u8>),
    BinHex(Vec<u8>),
    Uri(reqwest::Url),
    Uuid(uuid::Uuid),
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
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
            data_type => Err(anyhow::anyhow!("unrecognized data type: {data_type}")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Range {
    start: isize,
    end: isize,
    step: isize,
}

pub trait IntoStateVariable {
    const TYPE_NAME: &'static str;
    fn into_state_variable(&self) -> Value;
    fn default_value(&self) -> Option<Value> {
        None
    }
    fn value_list(&self) -> Option<Vec<String>> {
        None
    }
    fn value_range(&self) -> Option<Range> {
        None
    }
}

impl IntoStateVariable for u8 {
    const TYPE_NAME: &'static str = "ui1";
    fn into_state_variable(&self) -> Value {
        Value::Ui1(*self)
    }
}

impl IntoStateVariable for u16 {
    const TYPE_NAME: &'static str = "ui2";
    fn into_state_variable(&self) -> Value {
        Value::Ui2(*self)
    }
}

impl IntoStateVariable for u32 {
    const TYPE_NAME: &'static str = "ui4";
    fn into_state_variable(&self) -> Value {
        Value::Ui4(*self)
    }
}

impl IntoStateVariable for u64 {
    const TYPE_NAME: &'static str = "ui8";
    fn into_state_variable(&self) -> Value {
        Value::Ui8(*self)
    }
}

impl IntoStateVariable for i8 {
    const TYPE_NAME: &'static str = "i1";
    fn into_state_variable(&self) -> Value {
        Value::I1(*self)
    }
}

impl IntoStateVariable for i16 {
    const TYPE_NAME: &'static str = "i2";
    fn into_state_variable(&self) -> Value {
        Value::I2(*self)
    }
}

impl IntoStateVariable for i32 {
    const TYPE_NAME: &'static str = "i4";
    fn into_state_variable(&self) -> Value {
        Value::I4(*self)
    }
}

impl IntoStateVariable for i64 {
    const TYPE_NAME: &'static str = "i8";
    fn into_state_variable(&self) -> Value {
        Value::I8(*self)
    }
}

impl IntoStateVariable for bool {
    const TYPE_NAME: &'static str = "boolean";
    fn into_state_variable(&self) -> Value {
        Value::Boolean(*self)
    }
}

impl IntoStateVariable for uuid::Uuid {
    const TYPE_NAME: &'static str = "uuid";
    fn into_state_variable(&self) -> Value {
        Value::Uuid(*self)
    }
}


#[derive(Debug, Clone, Copy)]
enum VariableDirection {
    In,
    Out,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateVariable<T: IntoStateVariable> {
    name: String,
    #[serde(rename = "dataType")]
    kind: DataType,
    phantom: PhantomData<T>,
}
