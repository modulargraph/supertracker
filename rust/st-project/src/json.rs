//! Lenient, lossless conversion between JSON and the typed model.
//!
//! The interface owns the project and may write fields this crate does not know yet, and a
//! project file may hold a value of the wrong type. Reading therefore never fails on a field:
//!
//! * a field that is present and readable becomes `Some(value)`;
//! * a field that is absent stays `None`;
//! * a field that is `null` or has the wrong type stays in the object's `extra` map, verbatim,
//!   and reads as `None` (so its documented default applies, as in the interface);
//! * every key the model does not name stays in `extra`.
//!
//! Writing puts `extra` back and then every `Some` field, so `read -> write` reproduces the
//! input (object key order aside; numbers compare by value, see [`number`]).

use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;

/// A type with a JSON form. `from_json` returns the value unchanged when it cannot be read as
/// `Self`, so the caller can keep it.
pub trait Json: Sized {
    /// Reads `value`; `Err` gives it back untouched.
    fn from_json(value: Value) -> Result<Self, Value>;
    /// The JSON form.
    fn to_json(&self) -> Value;
}

/// A JSON number for `value`: whole numbers up to 2^53 are written as integers (as JavaScript's
/// `JSON.stringify` does), everything else as a float. A non-finite value becomes `null`
/// (validation rejects those before a project is saved).
pub fn number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Value::Number(Number::from(value as i64))
    } else {
        Number::from_f64(value).map(Value::Number).unwrap_or(Value::Null)
    }
}

/// Removes `key` from `map` and reads it; an unreadable value is put back.
pub(crate) fn take<T: Json>(map: &mut Map<String, Value>, key: &str) -> Option<T> {
    let value = map.remove(key)?;
    match T::from_json(value) {
        Ok(result) => Some(result),
        Err(original) => {
            map.insert(key.to_string(), original);
            None
        }
    }
}

impl Json for Value {
    fn from_json(value: Value) -> Result<Self, Value> {
        Ok(value)
    }
    fn to_json(&self) -> Value {
        self.clone()
    }
}

impl Json for String {
    fn from_json(value: Value) -> Result<Self, Value> {
        match value {
            Value::String(text) => Ok(text),
            other => Err(other),
        }
    }
    fn to_json(&self) -> Value {
        Value::String(self.clone())
    }
}

impl Json for bool {
    fn from_json(value: Value) -> Result<Self, Value> {
        match value {
            Value::Bool(flag) => Ok(flag),
            other => Err(other),
        }
    }
    fn to_json(&self) -> Value {
        Value::Bool(*self)
    }
}

impl Json for f64 {
    fn from_json(value: Value) -> Result<Self, Value> {
        match value.as_f64() {
            Some(number) if number.is_finite() => Ok(number),
            _ => Err(value),
        }
    }
    fn to_json(&self) -> Value {
        number(*self)
    }
}

/// Whole numbers only (`60` or `60.0`); a fractional number is not an integer field's value.
impl Json for i64 {
    fn from_json(value: Value) -> Result<Self, Value> {
        if let Some(integer) = value.as_i64() {
            return Ok(integer);
        }
        match value.as_f64() {
            Some(number) if number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_992.0 => Ok(number as i64),
            _ => Err(value),
        }
    }
    fn to_json(&self) -> Value {
        Value::Number(Number::from(*self))
    }
}

/// `null` is `None`; used for array elements and map values where `null` is meaningful
/// (an empty step, a disconnected output).
impl<T: Json> Json for Option<T> {
    fn from_json(value: Value) -> Result<Self, Value> {
        match value {
            Value::Null => Ok(None),
            other => T::from_json(other).map(Some),
        }
    }
    fn to_json(&self) -> Value {
        match self {
            Some(value) => value.to_json(),
            None => Value::Null,
        }
    }
}

/// An array whose every element is readable; otherwise the whole array is unreadable.
impl<T: Json> Json for Vec<T> {
    fn from_json(value: Value) -> Result<Self, Value> {
        let items = match value {
            Value::Array(items) => items,
            other => return Err(other),
        };
        // Check first, so a failure can hand the array back unchanged without cloning it up front.
        let mut result = Vec::with_capacity(items.len());
        let mut pending = items.into_iter();
        while let Some(item) = pending.next() {
            match T::from_json(item) {
                Ok(read) => result.push(read),
                Err(original) => {
                    let mut restored: Vec<Value> = result.iter().map(Json::to_json).collect();
                    restored.push(original);
                    restored.extend(pending);
                    return Err(Value::Array(restored));
                }
            }
        }
        Ok(result)
    }
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(Json::to_json).collect())
    }
}

/// An object whose every value is readable.
impl<T: Json> Json for BTreeMap<String, T> {
    fn from_json(value: Value) -> Result<Self, Value> {
        let map = match value {
            Value::Object(map) => map,
            other => return Err(other),
        };
        let mut result = BTreeMap::new();
        let mut pending = map.into_iter();
        while let Some((key, item)) = pending.next() {
            match T::from_json(item) {
                Ok(read) => {
                    result.insert(key, read);
                }
                Err(original) => {
                    let mut restored: Map<String, Value> = result.iter().map(|(key, value)| (key.clone(), value.to_json())).collect();
                    restored.insert(key, original);
                    restored.extend(pending);
                    return Err(Value::Object(restored));
                }
            }
        }
        Ok(result)
    }
    fn to_json(&self) -> Value {
        Value::Object(self.iter().map(|(key, value)| (key.clone(), value.to_json())).collect())
    }
}

/// Declares a JSON object of the project document.
///
/// Every listed field is `pub name: Option<Type>`; `extra` keeps the rest. A field declared with
/// `= default` also gets a method of the same name that returns the value or that default.
/// The struct implements [`Json`], `serde::Serialize` and `serde::Deserialize`.
macro_rules! model {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $key:literal => $field:ident : $ty:ty $(= $default:expr)?
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Default)]
        pub struct $name {
            $(
                $(#[$field_meta])*
                #[doc = concat!("\n\nJSON key `", $key, "`.")]
                pub $field: Option<$ty>,
            )*
            /// Everything else the object holds: fields this crate does not model, and values of
            /// modelled fields that were `null` or unreadable. Written back verbatim.
            pub extra: serde_json::Map<String, serde_json::Value>,
        }

        impl $name {
            $($(
                #[doc = concat!("`", $key, "`, or `", stringify!($default), "` when it is absent or unreadable.")]
                pub fn $field(&self) -> $ty {
                    self.$field.clone().unwrap_or($default)
                }
            )?)*
        }

        impl $crate::json::Json for $name {
            fn from_json(value: serde_json::Value) -> ::std::result::Result<Self, serde_json::Value> {
                let mut map = match value {
                    serde_json::Value::Object(map) => map,
                    other => return Err(other),
                };
                $( let $field = $crate::json::take::<$ty>(&mut map, $key); )*
                Ok(Self { $( $field, )* extra: map })
            }
            fn to_json(&self) -> serde_json::Value {
                let mut map = self.extra.clone();
                $(
                    if let Some(value) = &self.$field {
                        map.insert($key.to_string(), $crate::json::Json::to_json(value));
                    }
                )*
                serde_json::Value::Object(map)
            }
        }

        $crate::json::serde_through_json!($name, "a JSON object");
    };
}

/// Declares an enumeration stored as a JSON string. An unknown string is unreadable (it stays in
/// the owner's `extra` and the field's default applies).
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident = $text:literal ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $( $(#[$variant_meta])* #[doc = concat!("`\"", $text, "\"`")] $variant, )*
        }

        impl $name {
            /// The JSON string.
            pub fn as_str(self) -> &'static str {
                match self { $( $name::$variant => $text, )* }
            }
            /// The value for a JSON string, if it is one of this enumeration's.
            pub fn parse(text: &str) -> Option<Self> {
                match text { $( $text => Some($name::$variant), )* _ => None }
            }
        }

        impl $crate::json::Json for $name {
            fn from_json(value: serde_json::Value) -> ::std::result::Result<Self, serde_json::Value> {
                match value.as_str().and_then($name::parse) {
                    Some(result) => Ok(result),
                    None => Err(value),
                }
            }
            fn to_json(&self) -> serde_json::Value {
                serde_json::Value::String(self.as_str().to_string())
            }
        }

        $crate::json::serde_through_json!($name, "a known name");
    };
}

/// `serde` traits for a [`Json`] type, so the host can embed model types in its own payloads.
macro_rules! serde_through_json {
    ($name:ident, $expected:literal) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error> {
                serde::Serialize::serialize(&$crate::json::Json::to_json(self), serializer)
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> ::std::result::Result<Self, D::Error> {
                let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
                <$name as $crate::json::Json>::from_json(value)
                    .map_err(|_| <D::Error as serde::de::Error>::custom(concat!(stringify!($name), " must be ", $expected)))
            }
        }
    };
}

pub(crate) use {model, serde_through_json, string_enum};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    model! {
        /// A test object.
        pub struct Sample {
            /// A number with a default.
            "level" => level: f64 = 0.5,
            /// A whole number.
            "count" => count: i64 = 3,
            /// A list.
            "items" => items: Vec<Option<i64>>,
        }
    }

    string_enum! {
        /// A test enumeration.
        pub enum Shape {
            /// Round.
            Round = "round",
            /// Square.
            Square = "square",
        }
    }

    #[test]
    fn readable_fields_are_typed_and_the_rest_is_kept() {
        let value = json!({ "level": 2, "count": "many", "items": [1, null, 3], "other": { "a": 1 }, "nothing": null });
        let sample = Sample::from_json(value.clone()).unwrap();
        assert_eq!((sample.level, sample.count, sample.items.clone()), (Some(2.0), None, Some(vec![Some(1), None, Some(3)])));
        assert_eq!((sample.level(), sample.count()), (2.0, 3), "an unreadable value reads as the default");
        assert_eq!(sample.extra.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>(), ["count", "nothing", "other"]);
        assert_eq!(sample.to_json(), value);
        assert_eq!(Sample::from_json(json!([1])), Err(json!([1])), "not an object: handed back");
    }

    #[test]
    fn a_typed_value_replaces_an_unreadable_one() {
        let mut sample = Sample::from_json(json!({ "count": "many" })).unwrap();
        sample.count = Some(7);
        assert_eq!(sample.to_json(), json!({ "count": 7 }));
    }

    #[test]
    fn an_array_with_one_unreadable_element_is_handed_back_unchanged() {
        let value = json!([1, 2.0, "three", null, 5]);
        assert_eq!(Vec::<Option<i64>>::from_json(value.clone()), Err(json!([1, 2, "three", null, 5])));
        let sample = Sample::from_json(json!({ "items": value })).unwrap();
        assert!(sample.items.is_none() && sample.extra.contains_key("items"));
        let map = json!({ "a": 1, "b": "x", "c": 3 });
        assert_eq!(BTreeMap::<String, i64>::from_json(map.clone()), Err(map));
    }

    #[test]
    fn numbers() {
        assert_eq!(i64::from_json(json!(60.0)), Ok(60));
        assert_eq!(i64::from_json(json!(60.5)), Err(json!(60.5)));
        assert_eq!(i64::from_json(json!(true)), Err(json!(true)));
        assert_eq!(f64::from_json(json!(18446744073709551615u64)), Ok(18446744073709551615.0));
        assert_eq!(f64::from_json(json!("1")), Err(json!("1")));
        assert_eq!((number(120.0).to_string(), number(0.06).to_string(), number(-3.0).to_string(), number(1e300).to_string()), ("120".into(), "0.06".into(), "-3".into(), "1e+300".into()));
        assert_eq!(number(f64::NAN), Value::Null);
    }

    #[test]
    fn enumerations_and_serde() {
        assert_eq!(Shape::from_json(json!("square")), Ok(Shape::Square));
        assert_eq!(Shape::from_json(json!("oval")), Err(json!("oval")));
        assert_eq!((Shape::Round.as_str(), Shape::parse("round")), ("round", Some(Shape::Round)));
        let sample: Sample = serde_json::from_str(r#"{"level":1,"x":true}"#).unwrap();
        assert_eq!(serde_json::to_value(&sample).unwrap(), json!({ "level": 1, "x": true }));
        assert!(serde_json::from_str::<Sample>("[]").unwrap_err().to_string().contains("Sample must be a JSON object"));
        assert_eq!(serde_json::to_string(&Shape::Round).unwrap(), "\"round\"");
    }
}
