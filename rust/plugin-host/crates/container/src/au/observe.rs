//! What the backend observes on a unit (macOS only): key-value observation of the
//! properties a unit changes behind the host's back, and the parameter tree's observers.
//!
//! Everything here can be called on threads this crate does not own (a property changed
//! on a plug-in's worker thread, the tree's observer queue), so the callbacks only set
//! flags or push lock-free notifications, and ask for `Plugin::idle` on the main thread.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_audio_toolbox::{
    AUAudioUnit, AUParameterAutomationEvent, AUParameterAutomationEventType, AUParameterObserverToken, AUParameterTree,
};
use objc2_foundation::{NSDictionary, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSString};
use stpc_runtime::{HostHandle, Notification};

use super::objc::{guard_quiet, ns};

/// `AUAudioUnit.parameterTree` was replaced.
pub(super) const CHANGED_TREE: u32 = 1;
/// `AUAudioUnit.allParameterValues`: every value may have changed (a preset chosen in the
/// unit's own editor, say).
pub(super) const CHANGED_ALL_VALUES: u32 = 2;
/// `latency` or `tailTime`.
pub(super) const CHANGED_TIMING: u32 = 4;
/// The editor's view controller changed its `preferredContentSize`.
pub(super) const CHANGED_EDITOR_SIZE: u32 = 8;

const UNIT_KEYS: [(&str, u32); 4] = [
    ("parameterTree", CHANGED_TREE),
    ("allParameterValues", CHANGED_ALL_VALUES),
    ("latency", CHANGED_TIMING),
    ("tailTime", CHANGED_TIMING),
];
const EDITOR_KEY: &str = "preferredContentSize";

pub(super) struct Shared {
    changed: AtomicU32,
    host: HostHandle,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class does not implement Drop.
    // The only ivar is an `Arc` of atomics and the thread-safe host handle, so instances
    // may be messaged from any thread, which key-value observation does.
    #[unsafe(super(NSObject))]
    #[name = "STPCAudioUnitObserver"]
    #[ivars = Arc<Shared>]
    pub(super) struct Observer;

    unsafe impl NSObjectProtocol for Observer {}

    impl Observer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary>,
            context: *mut c_void,
        ) {
            // The context carries the flag (see `watch`); nothing is read from the change.
            let shared = self.ivars();
            shared.changed.fetch_or(context as usize as u32, Ordering::AcqRel);
            shared.host.request_main_callback();
        }
    }
);

impl Observer {
    pub fn new(host: HostHandle) -> Retained<Observer> {
        let this = Observer::alloc().set_ivars(Arc::new(Shared { changed: AtomicU32::new(0), host }));
        // SAFETY: plain NSObject initialisation.
        unsafe { msg_send![super(this), init] }
    }

    /// The changes seen since the last call.
    pub fn take_changes(&self) -> u32 {
        self.ivars().changed.swap(0, Ordering::AcqRel)
    }

    fn watch(&self, object: &NSObject, key: &str, flag: u32) {
        // SAFETY: the observer outlives the registration (`unwatch` is called before it
        // is released); the context is an integer, never dereferenced.
        guard_quiet("registering a key-value observer", || unsafe {
            object.addObserver_forKeyPath_options_context(
                self,
                &ns(key),
                NSKeyValueObservingOptions(0),
                flag as usize as *mut c_void,
            );
        });
    }

    fn unwatch(&self, object: &NSObject, key: &str, flag: u32) {
        // Removing an observer that was never added raises; that is all `guard` is for.
        // SAFETY: as in `watch`.
        guard_quiet("removing a key-value observer", || unsafe {
            object.removeObserver_forKeyPath_context(self, &ns(key), flag as usize as *mut c_void);
        });
    }

    pub fn watch_unit(&self, unit: &AUAudioUnit) {
        for (key, flag) in UNIT_KEYS {
            self.watch(unit, key, flag);
        }
    }

    pub fn unwatch_unit(&self, unit: &AUAudioUnit) {
        for (key, flag) in UNIT_KEYS {
            self.unwatch(unit, key, flag);
        }
    }

    /// `controller` is the editor's view controller.
    pub fn watch_editor(&self, controller: &NSObject) {
        self.watch(controller, EDITOR_KEY, CHANGED_EDITOR_SIZE);
    }

    pub fn unwatch_editor(&self, controller: &NSObject) {
        self.unwatch(controller, EDITOR_KEY, CHANGED_EDITOR_SIZE);
    }
}

type ValueObserver = RcBlock<dyn Fn(u64, f32)>;
type AutomationObserver = RcBlock<dyn Fn(isize, NonNull<AUParameterAutomationEvent>)>;

/// The observers installed on one parameter tree. Edits made in the unit's editor (or by
/// the unit itself) reach the host as `ParamValue` notifications, with gesture begin and
/// end where the unit reports touch and release.
pub(super) struct TreeObservation {
    tree: Retained<AUParameterTree>,
    value_token: AUParameterObserverToken,
    automation_token: AUParameterObserverToken,
    // The tree retains the blocks; these keep them alive should it not.
    _value_block: ValueObserver,
    _automation_block: AutomationObserver,
}

impl TreeObservation {
    pub fn install(tree: Retained<AUParameterTree>, host: &HostHandle) -> TreeObservation {
        let for_values = host.clone();
        // Called on an arbitrary thread; must not add or remove observers (it would
        // deadlock). `notify` is lock-free.
        let value_block: ValueObserver = RcBlock::new(move |address: u64, value: f32| {
            if value.is_finite() {
                for_values.notify(Notification::ParamValue { id: address, value: value as f64 });
            }
        });
        let for_gestures = host.clone();
        let automation_block: AutomationObserver =
            RcBlock::new(move |count: isize, events: NonNull<AUParameterAutomationEvent>| {
                for index in 0..count.clamp(0, 4096) as usize {
                    // SAFETY: the tree passes `count` events.
                    let event = unsafe { events.as_ptr().add(index).read_unaligned() };
                    // Value events arrive through the value observer as well; only the
                    // gesture boundaries are taken from here.
                    if event.eventType == AUParameterAutomationEventType::Touch {
                        for_gestures.notify(Notification::ParamGestureBegin { id: event.address });
                    } else if event.eventType == AUParameterAutomationEventType::Release {
                        for_gestures.notify(Notification::ParamGestureEnd { id: event.address });
                    }
                }
            });
        // SAFETY: main thread; the blocks are heap blocks that outlive the registration.
        let (value_token, automation_token) = guard_quiet("observing its parameters", || unsafe {
            (
                tree.tokenByAddingParameterObserver(RcBlock::as_ptr(&value_block)),
                tree.tokenByAddingParameterAutomationObserver(RcBlock::as_ptr(&automation_block)),
            )
        })
        .unwrap_or((std::ptr::null_mut(), std::ptr::null_mut()));
        TreeObservation {
            tree,
            value_token,
            automation_token,
            _value_block: value_block,
            _automation_block: automation_block,
        }
    }

    pub fn tree(&self) -> &AUParameterTree {
        &self.tree
    }

    /// Passed as the originator of the host's own writes, so that they are not echoed.
    pub fn originator(&self) -> AUParameterObserverToken {
        self.value_token
    }

    pub fn is_for(&self, tree: Option<&AUParameterTree>) -> bool {
        tree.is_some_and(|tree| std::ptr::eq(tree, &*self.tree))
    }
}

impl Drop for TreeObservation {
    fn drop(&mut self) {
        // Main thread, outside any observer callback.
        // SAFETY: the tokens came from this tree.
        guard_quiet("removing its parameter observers", || unsafe {
            if !self.value_token.is_null() {
                self.tree.removeParameterObserver(self.value_token);
            }
            if !self.automation_token.is_null() {
                self.tree.removeParameterObserver(self.automation_token);
            }
        });
    }
}
