//! A bounded MessagePack reader (<https://github.com/msgpack/msgpack/blob/master/spec.md>) that
//! produces JSON values.
//!
//! All standard scalar and container encodings are read. Binary and extension values become
//! JSON descriptors (`{"encoding": "base64", "data": ...}`); nothing is executed or interpreted.
//! Numbers that JSON cannot carry exactly (beyond 2^53) and non-finite floats become strings.

use crate::error::{Error, Result};
use crate::util::base64_encode;
use serde_json::{Map, Number, Value};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Reads values from a byte slice within fixed budgets: depth 32, 100,000 values, 1 MiB per
/// string or binary field.
pub struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
    values_remaining: usize,
}

fn invalid(message: &str) -> Error {
    Error::invalid(message)
}

impl<'a> Decoder<'a> {
    /// A decoder at the start of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Decoder { bytes, offset: 0, values_remaining: 100_000 }
    }

    /// Whether every byte was consumed.
    pub fn is_at_end(&self) -> bool {
        self.offset == self.bytes.len()
    }

    /// Reads the next value.
    pub fn decode(&mut self) -> Result<Value> {
        self.value(0)
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > 32 || self.values_remaining == 0 {
            return Err(invalid("MessagePack nesting or object budget exceeded."));
        }
        self.values_remaining -= 1;
        let marker = self.byte()?;
        Ok(match marker {
            0x00..=0x7f => Value::from(marker),
            0x80..=0x8f => self.map((marker & 0x0f) as usize, depth)?,
            0x90..=0x9f => self.array((marker & 0x0f) as usize, depth)?,
            0xa0..=0xbf => Value::String(self.string((marker & 0x1f) as usize)?),
            0xc0 => Value::Null,
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            0xc4..=0xc6 => {
                let length = self.unsigned(1 << (marker - 0xc4))? as usize;
                self.binary(length)?
            }
            0xc7..=0xc9 => {
                let length = self.unsigned(1 << (marker - 0xc7))? as usize;
                self.extension(length)?
            }
            0xca => float(f64::from(f32::from_bits(self.unsigned(4)? as u32))),
            0xcb => float(f64::from_bits(self.unsigned(8)?)),
            0xcc => Value::from(self.unsigned(1)?),
            0xcd => Value::from(self.unsigned(2)?),
            0xce => Value::from(self.unsigned(4)?),
            0xcf => {
                let value = self.unsigned(8)?;
                if value <= MAX_SAFE_INTEGER {
                    Value::from(value)
                } else {
                    Value::String(value.to_string())
                }
            }
            0xd0 => Value::from(self.byte()? as i8),
            0xd1 => Value::from(self.unsigned(2)? as u16 as i16),
            0xd2 => Value::from(self.unsigned(4)? as u32 as i32),
            0xd3 => {
                let value = self.unsigned(8)? as i64;
                if value.unsigned_abs() <= MAX_SAFE_INTEGER {
                    Value::from(value)
                } else {
                    Value::String(value.to_string())
                }
            }
            0xd4..=0xd8 => self.extension(1 << (marker - 0xd4))?,
            0xd9..=0xdb => {
                let length = self.unsigned(1 << (marker - 0xd9))? as usize;
                Value::String(self.string(length)?)
            }
            0xdc => {
                let count = self.unsigned(2)? as usize;
                self.array(count, depth)?
            }
            0xdd => {
                let count = self.unsigned(4)? as usize;
                self.array(count, depth)?
            }
            0xde => {
                let count = self.unsigned(2)? as usize;
                self.map(count, depth)?
            }
            0xdf => {
                let count = self.unsigned(4)? as usize;
                self.map(count, depth)?
            }
            0xe0..=0xff => Value::from(marker as i8),
            0xc1 => return Err(invalid("Reserved MessagePack marker 0xc1.")),
        })
    }

    fn byte(&mut self) -> Result<u8> {
        let value = *self.bytes.get(self.offset).ok_or_else(|| invalid("MessagePack data is truncated."))?;
        self.offset += 1;
        Ok(value)
    }

    fn unsigned(&mut self, count: usize) -> Result<u64> {
        let mut result = 0u64;
        for _ in 0..count {
            result = result << 8 | u64::from(self.byte()?);
        }
        Ok(result)
    }

    fn payload(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > 1_048_576 || length > self.bytes.len() - self.offset {
            return Err(invalid("MessagePack payload is truncated or exceeds the 1 MiB field limit."));
        }
        let result = &self.bytes[self.offset..self.offset + length];
        self.offset += length;
        Ok(result)
    }

    fn string(&mut self, length: usize) -> Result<String> {
        String::from_utf8(self.payload(length)?.to_vec()).map_err(|_| invalid("MessagePack string is not valid UTF-8."))
    }

    fn binary(&mut self, length: usize) -> Result<Value> {
        let data = base64_encode(self.payload(length)?);
        Ok(serde_json::json!({ "encoding": "base64", "data": data }))
    }

    fn extension(&mut self, length: usize) -> Result<Value> {
        let kind = self.byte()? as i8;
        let data = base64_encode(self.payload(length)?);
        Ok(serde_json::json!({ "extensionType": kind, "encoding": "base64", "data": data }))
    }

    fn array(&mut self, count: usize, depth: usize) -> Result<Value> {
        if count > 65_536 || count > self.bytes.len() - self.offset || count > self.values_remaining {
            return Err(invalid("MessagePack array exceeds its data or object budget."));
        }
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            result.push(self.value(depth + 1)?);
        }
        Ok(Value::Array(result))
    }

    fn map(&mut self, count: usize, depth: usize) -> Result<Value> {
        if count > 32_768 || count > (self.bytes.len() - self.offset) / 2 || count > self.values_remaining / 2 {
            return Err(invalid("MessagePack map exceeds its data or object budget."));
        }
        let mut result = Map::new();
        for _ in 0..count {
            let key = match self.value(depth + 1)? {
                Value::String(text) => text,
                Value::Number(number) => number.to_string(),
                _ => return Err(invalid("Unsupported MessagePack map key.")),
            };
            if result.contains_key(&key) {
                return Err(invalid("Duplicate MessagePack map key."));
            }
            let value = self.value(depth + 1)?;
            result.insert(key, value);
        }
        Ok(Value::Object(result))
    }
}

fn float(value: f64) -> Value {
    match Number::from_f64(value) {
        Some(number) => Value::Number(number),
        None => Value::String(if value.is_nan() { "nan".to_string() } else if value > 0.0 { "inf".to_string() } else { "-inf".to_string() }),
    }
}

/// A small MessagePack writer for fixtures: strings, maps (keys sorted), arrays, booleans,
/// integers (as 64-bit) and floats (as 64-bit).
#[doc(hidden)]
pub fn pack(value: &Value) -> Vec<u8> {
    match value {
        Value::Null => vec![0xc0],
        Value::Bool(flag) => vec![if *flag { 0xc3 } else { 0xc2 }],
        Value::String(text) => {
            let bytes = text.as_bytes();
            let mut data = if bytes.len() < 32 { vec![0xa0 | bytes.len() as u8] } else { vec![0xd9, bytes.len() as u8] };
            data.extend_from_slice(bytes);
            data
        }
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                let mut data = vec![if integer < 0 { 0xd3 } else { 0xcf }];
                data.extend_from_slice(&integer.to_be_bytes());
                data
            } else if let Some(integer) = number.as_u64() {
                let mut data = vec![0xcf];
                data.extend_from_slice(&integer.to_be_bytes());
                data
            } else {
                let mut data = vec![0xcb];
                data.extend_from_slice(&number.as_f64().unwrap_or(0.0).to_bits().to_be_bytes());
                data
            }
        }
        Value::Array(items) => {
            let mut data = if items.len() < 16 { vec![0x90 | items.len() as u8] } else { vec![0xdc, (items.len() >> 8) as u8, items.len() as u8] };
            for item in items {
                data.extend(pack(item));
            }
            data
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut data = if keys.len() < 16 { vec![0x80 | keys.len() as u8] } else { vec![0xde, (keys.len() >> 8) as u8, keys.len() as u8] };
            for key in keys {
                data.extend(pack(&Value::String(key.clone())));
                data.extend(pack(&map[key]));
            }
            data
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_fixture_values() {
        let value = json!({ "name": "Night Keys", "types": [["Keys", "Electric Piano"]], "id": 42, "negative": -7, "ratio": 0.5, "on": true, "none": null });
        let bytes = pack(&value);
        let mut decoder = Decoder::new(&bytes);
        assert_eq!(decoder.decode().unwrap(), value);
        assert!(decoder.is_at_end());
    }

    #[test]
    fn scalars_and_descriptors() {
        let decode = |bytes: &[u8]| Decoder::new(bytes).decode();
        assert_eq!(decode(&[0x2a]).unwrap(), json!(42));
        assert_eq!(decode(&[0xff]).unwrap(), json!(-1));
        assert_eq!(decode(&[0xce, 0x4b, 0x61, 0x74, 0x31]).unwrap(), json!(0x4b617431u32));
        assert_eq!(decode(&[0xd2, 0xff, 0xff, 0xff, 0xfe]).unwrap(), json!(-2));
        assert_eq!(decode(&[0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(), json!("18446744073709551615"));
        assert_eq!(decode(&[0xca, 0x7f, 0xc0, 0, 0]).unwrap(), json!("nan"));
        assert_eq!(decode(&[0xc4, 2, 1, 2]).unwrap(), json!({ "encoding": "base64", "data": "AQI=" }));
        assert_eq!(decode(&[0xd4, 5, 9]).unwrap(), json!({ "extensionType": 5, "encoding": "base64", "data": "CQ==" }));
        assert_eq!(decode(&[0x81, 0x01, 0xa1, b'x']).unwrap(), json!({ "1": "x" }));
    }

    #[test]
    fn malformed_input_is_rejected() {
        let mut nested = vec![0x91u8; 40];
        nested.push(0);
        let invalid: [&[u8]; 8] = [&[0xc1], &[0xdb, 0xff, 0xff, 0xff, 0xff], &[0xdd, 0xff, 0xff, 0xff, 0xff], &nested, &[0x81, 0xa1, 0x61], &[0xa1, 0xff], &[], &[0x82, 0xa1, b'a', 1, 0xa1, b'a', 2]];
        for (index, bytes) in invalid.iter().enumerate() {
            assert!(Decoder::new(bytes).decode().is_err(), "malformed MessagePack {index}");
        }
    }
}
