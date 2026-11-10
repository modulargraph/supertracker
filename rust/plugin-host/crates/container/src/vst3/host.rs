//! The host's own COM objects: what a plug-in is given to talk to.
//!
//! * [`HostApplication`]: `IHostApplication` (the context handed to `initialize`), which
//!   also makes the [`Message`] / [`Attributes`] objects plug-ins use to talk between
//!   their processor and controller.
//! * [`MemoryStream`]: `IBStream` over a byte vector, for state transfer.
//! * [`ComponentHandler`]: `IComponentHandler` + `IComponentHandler2`, the controller's
//!   line back to the host (edits made in the editor, restart requests, "dirty").

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use stpc_runtime::Notification;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper, Interface};

use super::process::Shared;

/// Largest state a plug-in may write into a stream.
pub const STATE_LIMIT: usize = 1 << 30;

fn copy_wide(text: &str, target: &mut [TChar]) {
    let mut len = 0;
    for unit in text.encode_utf16().take(target.len() - 1) {
        target[len] = unit;
        len += 1;
    }
    target[len] = 0;
}

fn guid_of(tuid: *const TUID) -> [u8; 16] {
    // SAFETY: callers pass the non-null id pointer they were given by the plug-in.
    let tuid = unsafe { &*tuid };
    std::array::from_fn(|index| tuid[index] as u8)
}

// ---- IHostApplication -----------------------------------------------------------------------

pub struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        copy_wide("SuperTracker", &mut *name);
        kResultOk
    }

    unsafe fn createInstance(&self, cid: *mut TUID, iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        if obj.is_null() || cid.is_null() || iid.is_null() {
            return kInvalidArgument;
        }
        *obj = std::ptr::null_mut();
        let (cid, iid) = (guid_of(cid), guid_of(iid));
        if cid == IMessage::IID && iid == IMessage::IID {
            *obj = ComWrapper::new(Message::new()).to_com_ptr::<IMessage>().unwrap().into_raw().cast();
            kResultOk
        } else if cid == IAttributeList::IID && iid == IAttributeList::IID {
            *obj = ComWrapper::new(Attributes::default()).to_com_ptr::<IAttributeList>().unwrap().into_raw().cast();
            kResultOk
        } else {
            kNoInterface
        }
    }
}

// ---- IMessage / IAttributeList ------------------------------------------------------------------

#[derive(Default)]
struct Value {
    integer: Option<i64>,
    float: Option<f64>,
    string: Option<Vec<TChar>>,
    binary: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct Attributes {
    values: RefCell<HashMap<Vec<u8>, Value>>,
}

impl Class for Attributes {
    type Interfaces = (IAttributeList,);
}

unsafe fn key(id: *const c_char) -> Option<Vec<u8>> {
    if id.is_null() {
        None
    } else {
        Some(CStr::from_ptr(id).to_bytes().to_vec())
    }
}

impl Attributes {
    unsafe fn update(&self, id: *const c_char, change: impl FnOnce(&mut Value)) -> tresult {
        match key(id) {
            Some(key) => {
                change(self.values.borrow_mut().entry(key).or_default());
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn read<T>(&self, id: *const c_char, pick: impl FnOnce(&Value) -> Option<T>) -> Option<T> {
        let key = key(id)?;
        self.values.borrow().get(&key).and_then(pick)
    }
}

impl IAttributeListTrait for Attributes {
    unsafe fn setInt(&self, id: IAttributeList_::AttrID, value: i64) -> tresult {
        self.update(id, |slot| slot.integer = Some(value))
    }

    unsafe fn getInt(&self, id: IAttributeList_::AttrID, value: *mut i64) -> tresult {
        match self.read(id, |slot| slot.integer) {
            Some(found) if !value.is_null() => {
                *value = found;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: IAttributeList_::AttrID, value: f64) -> tresult {
        self.update(id, |slot| slot.float = Some(value))
    }

    unsafe fn getFloat(&self, id: IAttributeList_::AttrID, value: *mut f64) -> tresult {
        match self.read(id, |slot| slot.float) {
            Some(found) if !value.is_null() => {
                *value = found;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: IAttributeList_::AttrID, string: *const TChar) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut len = 0;
        while len < (1 << 20) && *string.add(len) != 0 {
            len += 1;
        }
        let text = std::slice::from_raw_parts(string, len).to_vec();
        self.update(id, |slot| slot.string = Some(text))
    }

    unsafe fn getString(&self, id: IAttributeList_::AttrID, string: *mut TChar, size_in_bytes: u32) -> tresult {
        let capacity = size_in_bytes as usize / std::mem::size_of::<TChar>();
        if string.is_null() || capacity == 0 {
            return kInvalidArgument;
        }
        match self.read(id, |slot| slot.string.clone()) {
            Some(text) => {
                let count = text.len().min(capacity - 1);
                std::ptr::copy_nonoverlapping(text.as_ptr(), string, count);
                *string.add(count) = 0;
                kResultOk
            }
            None => kResultFalse,
        }
    }

    unsafe fn setBinary(&self, id: IAttributeList_::AttrID, data: *const c_void, size_in_bytes: u32) -> tresult {
        if data.is_null() && size_in_bytes != 0 {
            return kInvalidArgument;
        }
        let bytes = if size_in_bytes == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize).to_vec()
        };
        self.update(id, |slot| slot.binary = Some(bytes))
    }

    unsafe fn getBinary(&self, id: IAttributeList_::AttrID, data: *mut *const c_void, size_in_bytes: *mut u32) -> tresult {
        if data.is_null() || size_in_bytes.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = key(id) else { return kInvalidArgument };
        // The pointer stays valid until the attribute is set again, as the SDK's own list.
        match self.values.borrow().get(&key).and_then(|slot| slot.binary.as_ref()) {
            Some(bytes) => {
                *data = bytes.as_ptr().cast();
                *size_in_bytes = bytes.len() as u32;
                kResultOk
            }
            None => kResultFalse,
        }
    }
}

pub struct Message {
    id: RefCell<CString>,
    attributes: ComWrapper<Attributes>,
}

impl Message {
    fn new() -> Message {
        Message { id: RefCell::new(CString::default()), attributes: ComWrapper::new(Attributes::default()) }
    }
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        // Valid until the id is set again.
        self.id.borrow().as_ptr()
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        *self.id.borrow_mut() = if id.is_null() { CString::default() } else { CStr::from_ptr(id).to_owned() };
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        // Not add-ref'ed, by the interface's contract: the message owns the list.
        self.attributes.as_com_ref::<IAttributeList>().unwrap().as_ptr()
    }
}

// ---- IBStream -----------------------------------------------------------------------------------

/// A seekable stream over memory.
pub struct MemoryStream {
    bytes: RefCell<Vec<u8>>,
    position: Cell<usize>,
    writable: bool,
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl MemoryStream {
    /// An empty stream a plug-in writes its state into.
    pub fn writer() -> ComWrapper<MemoryStream> {
        ComWrapper::new(MemoryStream { bytes: RefCell::new(Vec::new()), position: Cell::new(0), writable: true })
    }

    /// A read-only stream over a copy of `bytes`.
    pub fn reader(bytes: &[u8]) -> ComWrapper<MemoryStream> {
        ComWrapper::new(MemoryStream { bytes: RefCell::new(bytes.to_vec()), position: Cell::new(0), writable: false })
    }

    pub fn rewind(&self) {
        self.position.set(0);
    }

    pub fn len(&self) -> usize {
        self.bytes.borrow().len()
    }

    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.bytes.borrow_mut())
    }

    pub fn contents(&self) -> Vec<u8> {
        self.bytes.borrow().clone()
    }
}

pub fn stream_ptr(stream: &ComWrapper<MemoryStream>) -> *mut IBStream {
    stream.as_com_ref::<IBStream>().unwrap().as_ptr()
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: i32, num_bytes_read: *mut i32) -> tresult {
        if !num_bytes_read.is_null() {
            *num_bytes_read = 0;
        }
        if num_bytes < 0 || (buffer.is_null() && num_bytes != 0) {
            return kInvalidArgument;
        }
        let bytes = self.bytes.borrow();
        let position = self.position.get().min(bytes.len());
        let count = (num_bytes as usize).min(bytes.len() - position);
        if count != 0 {
            std::ptr::copy_nonoverlapping(bytes.as_ptr().add(position), buffer as *mut u8, count);
        }
        self.position.set(position + count);
        if !num_bytes_read.is_null() {
            *num_bytes_read = count as i32;
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, num_bytes: i32, num_bytes_written: *mut i32) -> tresult {
        if !num_bytes_written.is_null() {
            *num_bytes_written = 0;
        }
        let position = self.position.get();
        if !self.writable
            || num_bytes < 0
            || (buffer.is_null() && num_bytes != 0)
            || position > STATE_LIMIT
            || num_bytes as usize > STATE_LIMIT - position
        {
            return kInvalidArgument;
        }
        let count = num_bytes as usize;
        let mut bytes = self.bytes.borrow_mut();
        if position + count > bytes.len() {
            bytes.resize(position + count, 0);
        }
        if count != 0 {
            std::ptr::copy_nonoverlapping(buffer as *const u8, bytes.as_mut_ptr().add(position), count);
        }
        self.position.set(position + count);
        if !num_bytes_written.is_null() {
            *num_bytes_written = num_bytes;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let base = match mode {
            0 => 0,
            1 => self.position.get() as i64,
            2 => self.bytes.borrow().len() as i64,
            _ => return kInvalidArgument,
        };
        let target = base.checked_add(pos).filter(|target| (0..=STATE_LIMIT as i64).contains(target));
        let Some(target) = target else { return kInvalidArgument };
        self.position.set(target as usize);
        if !result.is_null() {
            *result = target;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        *pos = self.position.get() as i64;
        kResultOk
    }
}

// ---- IComponentHandler ------------------------------------------------------------------------

/// What the plug-in's controller calls. Holds only the shared block, so the plug-in
/// keeping a reference to it creates no cycle.
pub struct ComponentHandler {
    pub shared: Arc<Shared>,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler, IComponentHandler2);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, id: ParamID) -> tresult {
        self.shared.handle.notify(Notification::ParamGestureBegin { id: id as u64 });
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        if !value.is_finite() {
            return kInvalidArgument;
        }
        // The host carries edits made in the editor to the processor.
        self.shared.edit(id, value.clamp(0.0, 1.0));
        kResultOk
    }

    unsafe fn endEdit(&self, id: ParamID) -> tresult {
        self.shared.handle.notify(Notification::ParamGestureEnd { id: id as u64 });
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: i32) -> tresult {
        // Handled from `Plugin::idle`: never inside the plug-in's own call.
        self.shared.restart_flags.fetch_or(flags as u32, Ordering::AcqRel);
        self.shared.handle.request_main_callback();
        kResultOk
    }
}

impl IComponentHandler2Trait for ComponentHandler {
    unsafe fn setDirty(&self, state: TBool) -> tresult {
        if state != 0 {
            self.shared.handle.notify(Notification::StateDirty);
        }
        kResultOk
    }

    unsafe fn requestOpenEditor(&self, _name: FIDString) -> tresult {
        kResultFalse
    }

    unsafe fn startGroupEdit(&self) -> tresult {
        kResultOk
    }

    unsafe fn finishGroupEdit(&self) -> tresult {
        kResultOk
    }
}
