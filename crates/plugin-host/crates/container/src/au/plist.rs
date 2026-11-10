//! The Objective-C side of state handling (macOS only): judging and serialising a state
//! dictionary, and reading one back. The framing and the canonical form are in
//! [`super::state`].

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, Message};
use objc2_core_foundation::{kCFBooleanFalse, kCFBooleanTrue};
use objc2_foundation::{
    NSArray, NSData, NSDate, NSDictionary, NSKeyedArchiveRootObjectKey, NSKeyedArchiver, NSKeyedUnarchiver,
    NSMutableArray, NSMutableDictionary, NSNumber, NSPropertyListFormat, NSPropertyListMutabilityOptions,
    NSPropertyListSerialization, NSRange, NSString,
};

use super::objc::{describe_error, guard};
use super::state::{compare_keys, Limits, PayloadKind, SemanticWriter};

/// Deeper structures are not walked (a keyed archive copes with them, and with cycles).
const MAX_DEPTH: usize = 200;
/// Longest string copied for the canonical form.
const MAX_STRING_UNITS: usize = 4 * 1024 * 1024;

pub(super) type StateDictionary = NSDictionary<NSString, AnyObject>;

pub(super) struct Captured {
    pub kind: PayloadKind,
    /// Values no encoding could carry were left out.
    pub pruned: bool,
    pub payload: Vec<u8>,
    /// Canonical form, if the state has one (see `state::SemanticWriter`).
    pub semantic: Option<Vec<u8>>,
}

struct Walk {
    /// Every value met so far is something a property list can hold.
    plist_safe: bool,
    writer: Option<SemanticWriter>,
}

impl Walk {
    fn write(&mut self, f: impl FnOnce(&mut SemanticWriter) -> Result<(), super::state::SemanticError>) {
        if let Some(writer) = &mut self.writer {
            if f(writer).is_err() {
                self.writer = None;
            }
        }
    }
}

fn is_boolean(object: &AnyObject) -> bool {
    let address = object as *const AnyObject as *const c_void;
    // SAFETY: reading immutable system constants. Booleans are the two CFBoolean
    // singletons; an NSNumber holding a char has the same encoding but another identity.
    let constants = unsafe { [kCFBooleanTrue, kCFBooleanFalse] };
    constants.iter().flatten().any(|constant| std::ptr::eq(*constant as *const _ as *const c_void, address))
}

fn utf16(string: &NSString) -> Option<Vec<u16>> {
    let length = string.length();
    if length > MAX_STRING_UNITS {
        return None;
    }
    let mut units = vec![0u16; length];
    if length > 0 {
        // SAFETY: the buffer holds exactly `length` units.
        unsafe { string.getCharacters_range(NonNull::new_unchecked(units.as_mut_ptr()), NSRange::new(0, length)) };
    }
    Some(units)
}

fn visit(object: &AnyObject, depth: usize, walk: &mut Walk) {
    if depth > MAX_DEPTH {
        walk.plist_safe = false;
        walk.writer = None;
        return;
    }
    walk.write(|w| w.node(depth));
    if let Some(string) = object.downcast_ref::<NSString>() {
        match utf16(string) {
            Some(units) if walk.writer.is_some() => walk.write(|w| w.string(&units)),
            Some(_) => {}
            None => walk.writer = None,
        }
    } else if let Some(number) = object.downcast_ref::<NSNumber>() {
        if is_boolean(object) {
            walk.write(|w| w.boolean(number.boolValue()));
        } else {
            // SAFETY: `objCType` is a NUL-terminated C string with at least the terminator.
            let code = unsafe { *number.objCType().as_ptr() } as u8;
            match code {
                b'f' => walk.write(|w| w.real(32, number.doubleValue())),
                b'd' => walk.write(|w| w.real(64, number.doubleValue())),
                b'Q' => walk.write(|w| w.unsigned(number.unsignedLongLongValue())),
                _ => walk.write(|w| w.signed(number.longLongValue())),
            }
        }
    } else if let Some(data) = object.downcast_ref::<NSData>() {
        // SAFETY: the bytes are only read inside this statement; nothing mutates the data.
        walk.write(|w| w.data(unsafe { data.as_bytes_unchecked() }));
    } else if let Some(date) = object.downcast_ref::<NSDate>() {
        walk.write(|w| w.date(date.timeIntervalSinceReferenceDate()));
    } else if let Some(array) = object.downcast_ref::<NSArray>() {
        let count = array.count();
        walk.write(|w| w.begin_array(count));
        for index in 0..count {
            visit(&array.objectAtIndex(index), depth + 1, walk);
            if !walk.plist_safe {
                return;
            }
        }
    } else if let Some(dictionary) = object.downcast_ref::<NSDictionary>() {
        let keys = dictionary.allKeys();
        let mut entries: Vec<(Vec<u16>, Retained<AnyObject>)> = Vec::with_capacity(keys.count());
        for index in 0..keys.count() {
            let key = keys.objectAtIndex(index);
            let Some(name) = key.downcast_ref::<NSString>() else {
                // A property list dictionary has string keys only.
                walk.plist_safe = false;
                walk.writer = None;
                return;
            };
            let Some(value) = dictionary.objectForKey(&key) else { continue };
            match utf16(name) {
                Some(units) => entries.push((units, value)),
                None => {
                    walk.writer = None;
                    entries.push((Vec::new(), value));
                }
            }
        }
        entries.sort_by(|a, b| compare_keys(&a.0, &b.0));
        walk.write(|w| w.begin_dictionary(entries.len()));
        for (key, value) in &entries {
            walk.write(|w| w.key(key));
            visit(value, depth + 1, walk);
            if !walk.plist_safe {
                return;
            }
        }
    } else {
        // NSNull, NSValue, NSSet, NSURL, a plug-in's own class, ...
        walk.plist_safe = false;
        walk.writer = None;
    }
}

fn analyse(object: &AnyObject) -> Walk {
    let mut walk = Walk { plist_safe: true, writer: Some(SemanticWriter::new(Limits::default())) };
    visit(object, 0, &mut walk);
    walk
}

fn upcast<T: Message>(object: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every object is an AnyObject.
    unsafe { Retained::cast_unchecked(object) }
}

/// A copy of `object` reduced to what a property list can hold.
fn prune(object: &AnyObject, depth: usize) -> Option<Retained<AnyObject>> {
    if depth > MAX_DEPTH {
        return None;
    }
    if let Some(array) = object.downcast_ref::<NSArray>() {
        let copy = NSMutableArray::<AnyObject>::new();
        for index in 0..array.count() {
            if let Some(item) = prune(&array.objectAtIndex(index), depth + 1) {
                copy.addObject(&item);
            }
        }
        Some(upcast(copy))
    } else if let Some(dictionary) = object.downcast_ref::<NSDictionary>() {
        let copy = NSMutableDictionary::<NSString, AnyObject>::new();
        let keys = dictionary.allKeys();
        for index in 0..keys.count() {
            let key = keys.objectAtIndex(index);
            let (Some(name), Some(value)) = (key.downcast_ref::<NSString>(), dictionary.objectForKey(&key)) else {
                continue;
            };
            if let Some(value) = prune(&value, depth + 1) {
                // SAFETY: the key is an immutable string (copied by the dictionary), the
                // value an object of this dictionary's value type.
                unsafe { copy.setObject_forKey(&value, ProtocolObject::from_ref(name)) };
            }
        }
        Some(upcast(copy))
    } else if object.downcast_ref::<NSString>().is_some()
        || object.downcast_ref::<NSNumber>().is_some()
        || object.downcast_ref::<NSData>().is_some()
        || object.downcast_ref::<NSDate>().is_some()
    {
        Some(object.retain())
    } else {
        None
    }
}

fn binary_plist(object: &AnyObject) -> Result<Vec<u8>, String> {
    // SAFETY: the serialiser validates the object graph and reports what it cannot write.
    let data = unsafe {
        NSPropertyListSerialization::dataWithPropertyList_format_options_error(
            object,
            NSPropertyListFormat::BinaryFormat_v1_0,
            0,
        )
    };
    data.map(|data| data.to_vec()).map_err(|error| describe_error(&error))
}

fn keyed_archive(object: &AnyObject) -> Result<Vec<u8>, String> {
    // An object that does not support coding raises; the caller's `guard` turns that into
    // an error.
    // SAFETY: archiving reads the object graph on this (the main) thread.
    let data = unsafe { NSKeyedArchiver::archivedDataWithRootObject_requiringSecureCoding_error(object, false) };
    data.map(|data| data.to_vec()).map_err(|error| describe_error(&error))
}

/// Serialises a state dictionary: as a binary property list if it is one; as a keyed
/// archive if it holds values a property list cannot (so that nothing is lost); and with
/// those values left out only if it cannot be archived either.
pub(super) fn capture(state: &StateDictionary) -> Result<Captured, String> {
    let object: &AnyObject = state;
    let walk = guard("reading its state", || analyse(object)).map_err(|e| e.to_string())?;
    let semantic = walk.writer.map(SemanticWriter::finish);
    let mut problems = Vec::new();
    if walk.plist_safe {
        match guard("saving its state", || binary_plist(object)).map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(payload) => return Ok(Captured { kind: PayloadKind::BinaryPlist, pruned: false, payload, semantic }),
            Err(reason) => problems.push(reason),
        }
    }
    match guard("archiving its state", || keyed_archive(object)).map_err(|e| e.to_string()).and_then(|r| r) {
        Ok(payload) => {
            return Ok(Captured { kind: PayloadKind::KeyedArchive, pruned: false, payload, semantic: None });
        }
        Err(reason) => problems.push(reason),
    }
    let pruned = guard("reducing its state", || prune(object, 0)).map_err(|e| e.to_string())?;
    let pruned = pruned.ok_or_else(|| "the state is not a dictionary".to_string())?;
    match guard("saving its reduced state", || binary_plist(&pruned)).map_err(|e| e.to_string()).and_then(|r| r) {
        Ok(payload) => {
            eprintln!(
                "[au] a plug-in's state holds values that cannot be saved; they were left out ({})",
                problems.join("; ")
            );
            Ok(Captured { kind: PayloadKind::BinaryPlist, pruned: true, payload, semantic: None })
        }
        Err(reason) => {
            problems.push(reason);
            Err(format!("The Audio Unit's state cannot be saved: {}", problems.join("; ")))
        }
    }
}

pub(super) struct Decoded {
    pub dictionary: Retained<StateDictionary>,
    pub semantic: Option<Vec<u8>>,
}

fn as_state(object: Retained<AnyObject>) -> Result<Decoded, String> {
    let Some(dictionary) = object.downcast_ref::<NSDictionary>() else {
        return Err("The saved Audio Unit state is not a dictionary.".to_string());
    };
    let keys = dictionary.allKeys();
    if (0..keys.count()).any(|index| keys.objectAtIndex(index).downcast_ref::<NSString>().is_none()) {
        return Err("The saved Audio Unit state has keys that are not strings.".to_string());
    }
    let walk = analyse(&object);
    let semantic = if walk.plist_safe { walk.writer.map(SemanticWriter::finish) } else { None };
    // SAFETY: checked above: a dictionary whose keys are all strings.
    let dictionary = unsafe { Retained::cast_unchecked::<StateDictionary>(object) };
    Ok(Decoded { dictionary, semantic })
}

/// Reads a property list (binary or XML) whose root must be a dictionary.
pub(super) fn decode_plist(data: &[u8]) -> Result<Decoded, String> {
    guard("reading a saved state", || {
        let data = NSData::with_bytes(data);
        // SAFETY: the parser validates its input.
        let object = unsafe {
            NSPropertyListSerialization::propertyListWithData_options_format_error(
                &data,
                NSPropertyListMutabilityOptions::Immutable,
                std::ptr::null_mut(),
            )
        }
        .map_err(|error| {
            format!("The saved Audio Unit state is not a valid property list: {}", describe_error(&error))
        })?;
        as_state(object)
    })
    .map_err(|e| e.to_string())?
}

/// Reads a keyed archive written by [`capture`]. Decoding instantiates the classes the
/// plug-in archived (the plug-in is loaded in this process, so they exist); the bytes come
/// from the project the user opened, as the plug-in's code does.
pub(super) fn decode_archive(data: &[u8]) -> Result<Decoded, String> {
    guard("reading a saved state", || {
        let data = NSData::with_bytes(data);
        // SAFETY: the unarchiver validates the archive; class restrictions are lifted on
        // purpose, see above.
        let object = unsafe {
            let unarchiver = NSKeyedUnarchiver::initForReadingFromData_error(NSKeyedUnarchiver::alloc(), &data)
                .map_err(|error| format!("The saved Audio Unit state is damaged: {}", describe_error(&error)))?;
            unarchiver.setRequiresSecureCoding(false);
            let object = unarchiver.decodeObjectForKey(NSKeyedArchiveRootObjectKey);
            unarchiver.finishDecoding();
            object
        };
        as_state(object.ok_or_else(|| "The saved Audio Unit state is damaged.".to_string())?)
    })
    .map_err(|e| e.to_string())?
}
