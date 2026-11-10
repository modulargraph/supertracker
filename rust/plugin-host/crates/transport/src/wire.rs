//! Little-endian encoding of control messages. Hand-written (no serde): the protocol is
//! small, and every decoder bounds-checks, so a corrupt or hostile peer yields an error,
//! never a panic or an oversized allocation.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    Truncated,
    BadUtf8,
    BadValue(&'static str),
    UnknownOpcode(u16),
    Trailing(usize),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Truncated => write!(f, "message truncated"),
            WireError::BadUtf8 => write!(f, "string is not UTF-8"),
            WireError::BadValue(what) => write!(f, "invalid value for {what}"),
            WireError::UnknownOpcode(code) => write!(f, "unknown opcode {code}"),
            WireError::Trailing(n) => write!(f, "{n} unexpected trailing bytes"),
        }
    }
}
impl std::error::Error for WireError {}

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::with_capacity(64) }
    }
    pub fn bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.remaining() < n {
            return Err(WireError::Truncated);
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    pub fn finish(&self) -> Result<(), WireError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(WireError::Trailing(n)),
        }
    }
}

pub trait Wire: Sized {
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader<'_>) -> Result<Self, WireError>;

    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.put(&mut w);
        w.buf
    }
    fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes);
        let value = Self::get(&mut r)?;
        r.finish()?;
        Ok(value)
    }
}

macro_rules! wire_num {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn put(&self, w: &mut Writer) { w.bytes(&self.to_le_bytes()); }
            fn get(r: &mut Reader<'_>) -> Result<Self, WireError> {
                let raw = r.take(std::mem::size_of::<$t>())?;
                Ok(<$t>::from_le_bytes(raw.try_into().unwrap()))
            }
        }
    )*};
}
wire_num!(u8, u16, u32, u64, i32, i64, f32, f64);

impl Wire for bool {
    fn put(&self, w: &mut Writer) {
        w.bytes(&[*self as u8]);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, WireError> {
        match u8::get(r)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WireError::BadValue("bool")),
        }
    }
}

impl Wire for String {
    fn put(&self, w: &mut Writer) {
        (self.len() as u32).put(w);
        w.bytes(self.as_bytes());
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let len = u32::get(r)? as usize;
        let raw = r.take(len)?;
        String::from_utf8(raw.to_vec()).map_err(|_| WireError::BadUtf8)
    }
}

/// Opaque bytes (plug-in state). A newtype so that `Vec<T>` can stay generic.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Wire for Bytes {
    fn put(&self, w: &mut Writer) {
        (self.0.len() as u64).put(w);
        w.bytes(&self.0);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let len = u64::get(r)?;
        if len > r.remaining() as u64 {
            return Err(WireError::Truncated);
        }
        Ok(Bytes(r.take(len as usize)?.to_vec()))
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, w: &mut Writer) {
        (self.len() as u32).put(w);
        for item in self {
            item.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let len = u32::get(r)? as usize;
        // Every element takes at least one byte: a forged length cannot force a huge allocation.
        if len > r.remaining() {
            return Err(WireError::Truncated);
        }
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(T::get(r)?);
        }
        Ok(out)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            None => false.put(w),
            Some(value) => {
                true.put(w);
                value.put(w);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(if bool::get(r)? { Some(T::get(r)?) } else { None })
    }
}

/// Declares a plain struct and its [`Wire`] implementation (fields in declaration order).
#[macro_export]
macro_rules! wire_struct {
    ($(#[$m:meta])* pub struct $name:ident { $($(#[$fm:meta])* pub $f:ident : $t:ty),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Default)]
        pub struct $name { $($(#[$fm])* pub $f: $t),* }
        impl $crate::wire::Wire for $name {
            fn put(&self, w: &mut $crate::wire::Writer) { $($crate::wire::Wire::put(&self.$f, w);)* }
            fn get(r: &mut $crate::wire::Reader<'_>) -> Result<Self, $crate::wire::WireError> {
                Ok($name { $($f: $crate::wire::Wire::get(r)?),* })
            }
        }
    };
}

/// Declares an enum of struct-like variants, each with a fixed `u16` opcode.
#[macro_export]
macro_rules! wire_enum {
    ($(#[$m:meta])* pub enum $name:ident {
        $($(#[$vm:meta])* $v:ident { $($f:ident : $t:ty),* $(,)? } = $code:literal),* $(,)?
    }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq)]
        pub enum $name { $($(#[$vm])* $v { $($f: $t),* }),* }
        impl $crate::wire::Wire for $name {
            fn put(&self, w: &mut $crate::wire::Writer) {
                match self {
                    $($name::$v { $($f),* } => {
                        $crate::wire::Wire::put(&($code as u16), w);
                        $($crate::wire::Wire::put($f, w);)*
                    }),*
                }
            }
            fn get(r: &mut $crate::wire::Reader<'_>) -> Result<Self, $crate::wire::WireError> {
                let code = <u16 as $crate::wire::Wire>::get(r)?;
                match code {
                    $($code => Ok($name::$v { $($f: $crate::wire::Wire::get(r)?),* }),)*
                    other => Err($crate::wire::WireError::UnknownOpcode(other)),
                }
            }
        }
        impl $name {
            /// Variant name, for logs and error messages.
            pub fn name(&self) -> &'static str {
                match self { $($name::$v { .. } => stringify!($v)),* }
            }
        }
    };
}
