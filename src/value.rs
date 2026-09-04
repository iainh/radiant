use std::collections::{BTreeMap, HashMap};

use crate::{SafeHtml, SafeJsonString, SafeXml};

/// A value understood by the dynamic evaluator.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Sequence(Vec<Self>),
    Map(BTreeMap<String, Self>),
    SafeHtml(String),
    SafeXml(String),
    SafeJsonString(String),
}

impl Value {
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Integer(value) => *value != 0,
            Self::Float(value) => *value != 0.0,
            Self::String(value)
            | Self::SafeHtml(value)
            | Self::SafeXml(value)
            | Self::SafeJsonString(value) => !value.is_empty(),
            Self::Sequence(value) => !value.is_empty(),
            Self::Map(value) => !value.is_empty(),
        }
    }

    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Integer(_) => "integer",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Sequence(_) => "sequence",
            Self::Map(_) => "map",
            Self::SafeHtml(_) => "safe HTML",
            Self::SafeXml(_) => "safe XML",
            Self::SafeJsonString(_) => "safe JSON string",
        }
    }

    /// Converts a Serde value into Radiant's explicit dynamic value model.
    ///
    /// This adapter is available with the `serde` feature. Checked templates
    /// should normally prefer [`TemplateValue`] so exposed fields remain
    /// explicit.
    #[cfg(feature = "serde")]
    pub fn from_serialize(value: &impl serde::Serialize) -> Result<Self, serde_json::Error> {
        serde_json::to_value(value).map(Self::from_json)
    }

    #[cfg(feature = "serde")]
    fn from_json(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(value),
            serde_json::Value::Number(value) => value.as_i64().map_or_else(
                || Self::Float(value.as_f64().unwrap_or_default()),
                Self::Integer,
            ),
            serde_json::Value::String(value) => Self::String(value),
            serde_json::Value::Array(values) => {
                Self::Sequence(values.into_iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::Map(
                values
                    .into_iter()
                    .map(|(key, value)| (key, Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    pub(crate) fn plain_text(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Bool(value) => value.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::String(value)
            | Self::SafeHtml(value)
            | Self::SafeXml(value)
            | Self::SafeJsonString(value) => value.clone(),
            Self::Sequence(_) => "[sequence]".into(),
            Self::Map(_) => "[map]".into(),
        }
    }
}

/// Converts application data into the explicit template value model.
pub trait IntoValue {
    fn into_value(self) -> Value;
}

/// Exposes fields of a Rust type to dynamic templates.
pub trait TemplateValue {
    fn to_value(&self) -> Value;
}

impl IntoValue for Value {
    fn into_value(self) -> Value {
        self
    }
}

impl IntoValue for &Value {
    fn into_value(self) -> Value {
        self.clone()
    }
}

impl IntoValue for () {
    fn into_value(self) -> Value {
        Value::Null
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

impl IntoValue for &bool {
    fn into_value(self) -> Value {
        Value::Bool(*self)
    }
}

macro_rules! integers {
    ($($ty:ty),* $(,)?) => {$
        (
            impl IntoValue for $ty {
                fn into_value(self) -> Value { Value::Integer(self as i64) }
            }
            impl IntoValue for &$ty {
                fn into_value(self) -> Value { Value::Integer(*self as i64) }
            }
        )*
    };
}

integers!(i8, i16, i32, i64, isize, u8, u16, u32);

impl IntoValue for usize {
    fn into_value(self) -> Value {
        i64::try_from(self).map_or_else(|_| Value::Float(self as f64), Value::Integer)
    }
}

impl IntoValue for &usize {
    fn into_value(self) -> Value {
        (*self).into_value()
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        i64::try_from(self).map_or_else(|_| Value::Float(self as f64), Value::Integer)
    }
}

impl IntoValue for &u64 {
    fn into_value(self) -> Value {
        (*self).into_value()
    }
}

macro_rules! floats {
    ($($ty:ty),* $(,)?) => {$
        (
            impl IntoValue for $ty {
                fn into_value(self) -> Value { Value::Float(self as f64) }
            }
            impl IntoValue for &$ty {
                fn into_value(self) -> Value { Value::Float(*self as f64) }
            }
        )*
    };
}

floats!(f32, f64);

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }
}

impl IntoValue for &String {
    fn into_value(self) -> Value {
        Value::String(self.clone())
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Value {
        Value::String(self.into())
    }
}

impl<T: ?Sized> IntoValue for &&T
where
    for<'a> &'a T: IntoValue,
{
    fn into_value(self) -> Value {
        (*self).into_value()
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        self.map_or(Value::Null, IntoValue::into_value)
    }
}

impl<T> IntoValue for &Option<T>
where
    for<'a> &'a T: IntoValue,
{
    fn into_value(self) -> Value {
        self.as_ref().map_or(Value::Null, IntoValue::into_value)
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::Sequence(self.into_iter().map(IntoValue::into_value).collect())
    }
}

impl<T> IntoValue for &Vec<T>
where
    for<'a> &'a T: IntoValue,
{
    fn into_value(self) -> Value {
        Value::Sequence(self.iter().map(IntoValue::into_value).collect())
    }
}

impl<T> IntoValue for &[T]
where
    for<'a> &'a T: IntoValue,
{
    fn into_value(self) -> Value {
        Value::Sequence(self.iter().map(IntoValue::into_value).collect())
    }
}

impl<T: IntoValue> IntoValue for BTreeMap<String, T> {
    fn into_value(self) -> Value {
        Value::Map(
            self.into_iter()
                .map(|(key, value)| (key, value.into_value()))
                .collect(),
        )
    }
}

impl<T: IntoValue> IntoValue for HashMap<String, T> {
    fn into_value(self) -> Value {
        Value::Map(
            self.into_iter()
                .map(|(key, value)| (key, value.into_value()))
                .collect(),
        )
    }
}

impl IntoValue for SafeHtml {
    fn into_value(self) -> Value {
        Value::SafeHtml(self.into_inner())
    }
}

impl IntoValue for &SafeHtml {
    fn into_value(self) -> Value {
        Value::SafeHtml(self.as_str().into())
    }
}

impl IntoValue for SafeXml {
    fn into_value(self) -> Value {
        Value::SafeXml(self.into_inner())
    }
}

impl IntoValue for &SafeXml {
    fn into_value(self) -> Value {
        Value::SafeXml(self.as_str().into())
    }
}

impl IntoValue for SafeJsonString {
    fn into_value(self) -> Value {
        Value::SafeJsonString(self.into_inner())
    }
}

impl IntoValue for &SafeJsonString {
    fn into_value(self) -> Value {
        Value::SafeJsonString(self.as_str().into())
    }
}
