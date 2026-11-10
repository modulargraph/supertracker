//! Loading a VST3 module and reading its class factory.
//!
//! A VST3 plug-in is a bundle directory `Name.vst3` holding one shared library per
//! platform (Windows also allows a bare `Name.vst3` DLL):
//!
//! | Platform | Library | Entry / exit |
//! |---|---|---|
//! | Linux and other Unix | `Contents/<arch>-linux/Name.so` | `ModuleEntry(handle)` / `ModuleExit()` |
//! | macOS | the bundle executable (`Contents/MacOS/…`), through `CFBundle` | `bundleEntry(CFBundleRef)` / `bundleExit()` |
//! | Windows | `Contents/<arch>-win/Name.vst3` | `InitDll()` / `ExitDll()` |
//!
//! Every library exports `GetPluginFactory()`. Only the Linux path has run. The macOS path
//! is type-checked with the workspace; the Windows path is type-checked on its own (this
//! file alone, `--target x86_64-pc-windows-gnu`), because the workspace is Unix only.
//!
//! This file depends on `std`, `libc` and the `vst3` bindings only.

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};

use vst3::Steinberg::*;
use vst3::{ComPtr, Interface};

/// The category of classes that are plug-ins ("Audio Module Class").
pub const AUDIO_MODULE_CATEGORY: &str = "Audio Module Class";

/// One class of a module's factory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassInfo {
    pub cid: TUID,
    /// "Audio Module Class" (a plug-in), "Component Controller Class", ...
    pub category: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sdk_version: String,
    /// "Fx|Dynamics", "Instrument|Synth", ...
    pub sub_categories: String,
    pub class_flags: u32,
}

impl ClassInfo {
    pub fn is_audio_module(&self) -> bool {
        self.category == AUDIO_MODULE_CATEGORY
    }
}

/// What the factory says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FactoryInfo {
    pub vendor: String,
    pub url: String,
    pub email: String,
}

/// The four 32-bit words of a class id in the order hosts print them (`FUID::toString`).
/// The byte layout of a `TUID` differs between Windows (COM GUID layout) and the rest.
pub fn cid_words(cid: &TUID) -> [u32; 4] {
    let b: [u8; 16] = std::array::from_fn(|index| cid[index] as u8);
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    if cfg!(target_os = "windows") {
        [u32::from_le_bytes([b[0], b[1], b[2], b[3]]), u32::from_be_bytes([b[5], b[4], b[7], b[6]]), be(8), be(12)]
    } else {
        [be(0), be(4), be(8), be(12)]
    }
}

/// A class id as 32 upper-case hex digits.
pub fn cid_to_hex(cid: &TUID) -> String {
    let words = cid_words(cid);
    format!("{:08X}{:08X}{:08X}{:08X}", words[0], words[1], words[2], words[3])
}

/// Parses 32 hex digits (either case; braces and dashes are ignored).
pub fn hex_to_cid(text: &str) -> Option<TUID> {
    let digits: String = text.chars().filter(|c| !matches!(c, '{' | '}' | '-' | ' ')).collect();
    if digits.len() != 32 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let word = |index: usize| u32::from_str_radix(&digits[index * 8..index * 8 + 8], 16).ok();
    Some(vst3::uid(word(0)?, word(1)?, word(2)?, word(3)?))
}

pub fn chars_to_string(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn wide_to_string(chars: &[u16]) -> String {
    let len = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..len])
}

/// A loaded module. Dropping it releases the factory, calls the module's exit function and
/// unloads the library, in that order.
pub struct Module {
    factory: Option<ComPtr<IPluginFactory>>,
    library: Option<imp::Library>,
    path: PathBuf,
}

impl Module {
    /// Loads the module at `path` (the `.vst3` bundle, or the library itself), runs its
    /// entry function and fetches the factory. `host_context` is offered to an
    /// `IPluginFactory3` and must outlive the module.
    ///
    /// # Safety
    /// Runs foreign code (library initialisers, the module entry).
    pub unsafe fn load(path: &Path, host_context: *mut FUnknown) -> Result<Module, String> {
        let library = imp::Library::open(path)?;
        // From here on `module` cleans up on every error path.
        let mut module = Module { factory: None, library: Some(library), path: path.to_path_buf() };
        let get_factory = module.library.as_ref().unwrap().symbol("GetPluginFactory");
        if get_factory.is_null() {
            return Err(format!("{} is not a VST3 module (no GetPluginFactory)", path.display()));
        }
        let get_factory: unsafe extern "system" fn() -> *mut IPluginFactory = std::mem::transmute(get_factory);
        // The SDK hands out the factory with a reference for the caller.
        let Some(factory) = ComPtr::from_raw(get_factory()) else {
            return Err(format!("{} has no plug-in factory", path.display()));
        };
        if !host_context.is_null() {
            if let Some(factory3) = factory.cast::<IPluginFactory3>() {
                factory3.setHostContext(host_context);
            }
        }
        module.factory = Some(factory);
        Ok(module)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn factory(&self) -> &ComPtr<IPluginFactory> {
        self.factory.as_ref().expect("a loaded module has a factory")
    }

    /// `CFBundleShortVersionString` of the bundle on macOS; empty elsewhere.
    pub fn bundle_version(&self) -> String {
        self.library.as_ref().map(|library| library.bundle_version()).unwrap_or_default()
    }

    pub fn factory_info(&self) -> FactoryInfo {
        // SAFETY: plain call on the live factory with a valid out-pointer.
        unsafe {
            let mut info: PFactoryInfo = std::mem::zeroed();
            if self.factory().getFactoryInfo(&mut info) != kResultOk {
                return FactoryInfo::default();
            }
            FactoryInfo {
                vendor: chars_to_string(&info.vendor),
                url: chars_to_string(&info.url),
                email: chars_to_string(&info.email),
            }
        }
    }

    /// All classes of the factory, with the richest class info each one offers
    /// (`PClassInfoW`, else `PClassInfo2`, else `PClassInfo`). A class without a vendor
    /// gets the factory's.
    pub fn classes(&self) -> Vec<ClassInfo> {
        let factory = self.factory();
        let factory_vendor = self.factory_info().vendor;
        let factory2 = factory.cast::<IPluginFactory2>();
        let factory3 = factory.cast::<IPluginFactory3>();
        let mut classes = Vec::new();
        // SAFETY: calls on the live factory with valid out-pointers; all strings are
        // read with their array bounds.
        unsafe {
            for index in 0..factory.countClasses().clamp(0, 16384) {
                let mut class = None;
                if let Some(factory3) = &factory3 {
                    let mut info: PClassInfoW = std::mem::zeroed();
                    if factory3.getClassInfoUnicode(index, &mut info) == kResultOk {
                        class = Some(ClassInfo {
                            cid: info.cid,
                            category: chars_to_string(&info.category),
                            name: wide_to_string(&info.name),
                            vendor: wide_to_string(&info.vendor),
                            version: wide_to_string(&info.version),
                            sdk_version: wide_to_string(&info.sdkVersion),
                            sub_categories: chars_to_string(&info.subCategories),
                            class_flags: info.classFlags,
                        });
                    }
                }
                if let (None, Some(factory2)) = (&class, &factory2) {
                    let mut info: PClassInfo2 = std::mem::zeroed();
                    if factory2.getClassInfo2(index, &mut info) == kResultOk {
                        class = Some(ClassInfo {
                            cid: info.cid,
                            category: chars_to_string(&info.category),
                            name: chars_to_string(&info.name),
                            vendor: chars_to_string(&info.vendor),
                            version: chars_to_string(&info.version),
                            sdk_version: chars_to_string(&info.sdkVersion),
                            sub_categories: chars_to_string(&info.subCategories),
                            class_flags: info.classFlags,
                        });
                    }
                }
                if class.is_none() {
                    let mut info: PClassInfo = std::mem::zeroed();
                    if factory.getClassInfo(index, &mut info) == kResultOk {
                        class = Some(ClassInfo {
                            cid: info.cid,
                            category: chars_to_string(&info.category),
                            name: chars_to_string(&info.name),
                            ..ClassInfo::default()
                        });
                    }
                }
                if let Some(mut class) = class {
                    if class.vendor.is_empty() {
                        class.vendor = factory_vendor.clone();
                    }
                    classes.push(class);
                }
            }
        }
        classes
    }

    /// Creates an instance of class `cid` and returns its interface `I`.
    ///
    /// # Safety
    /// Runs the plug-in's constructor.
    pub unsafe fn create<I: Interface>(&self, cid: &TUID) -> Option<ComPtr<I>> {
        let mut object: *mut c_void = std::ptr::null_mut();
        let iid = I::IID;
        let result = self.factory().createInstance(cid.as_ptr(), iid.as_ptr() as *const c_char, &mut object);
        if result != kResultOk {
            return None;
        }
        ComPtr::from_raw(object as *mut I)
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // Order matters: objects of the module (the factory) go before its exit function,
        // the exit function before the code is unmapped.
        self.factory = None;
        self.library = None;
    }
}

/// Directory names a bundle may use for this machine's architecture.
#[cfg(not(target_os = "macos"))]
fn architecture_names() -> &'static [&'static str] {
    match std::env::consts::ARCH {
        "x86_64" => &["x86_64"],
        "x86" => &["i386", "i686", "x86"],
        "aarch64" => &["aarch64", "arm64", "arm64ec"],
        "arm" => &["armv7l", "armv8l", "arm"],
        "riscv64" => &["riscv64"],
        _ => {
            const NATIVE: &[&str] = &[std::env::consts::ARCH];
            NATIVE
        }
    }
}

/// The shared library inside a bundle, by the naming rules of this platform.
#[cfg(not(target_os = "macos"))]
pub fn library_in_bundle(bundle: &Path) -> Result<PathBuf, String> {
    if !bundle.is_dir() {
        return Ok(bundle.to_path_buf());
    }
    let stem = bundle.file_stem().ok_or_else(|| format!("bad VST3 bundle path {}", bundle.display()))?;
    let (system, extension) = if cfg!(target_os = "windows") { ("win", "vst3") } else { ("linux", "so") };
    let mut tried = Vec::new();
    for architecture in architecture_names() {
        let mut file = stem.to_os_string();
        file.push(".");
        file.push(extension);
        let candidate = bundle.join("Contents").join(format!("{architecture}-{system}")).join(file);
        if candidate.is_file() {
            return Ok(candidate);
        }
        tried.push(candidate.display().to_string());
    }
    Err(format!("{} has no library for this machine (looked for {})", bundle.display(), tried.join(", ")))
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use std::ffi::{CStr, CString};

    pub struct Library {
        handle: *mut c_void,
        entered: bool,
    }

    impl Library {
        pub unsafe fn open(path: &Path) -> Result<Library, String> {
            let binary = library_in_bundle(path)?;
            let c_binary = CString::new(binary.to_string_lossy().as_bytes()).map_err(|_| "bad path".to_string())?;
            let handle = libc::dlopen(c_binary.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if handle.is_null() {
                let reason = libc::dlerror();
                let reason =
                    if reason.is_null() { "unknown error".into() } else { CStr::from_ptr(reason).to_string_lossy() };
                return Err(format!("cannot load {}: {reason}", binary.display()));
            }
            let mut library = Library { handle, entered: false };
            // The SDK's own host insists on ModuleEntry; a library without it but with a
            // factory is accepted here.
            let entry = library.symbol("ModuleEntry");
            if !entry.is_null() {
                let entry: unsafe extern "system" fn(*mut c_void) -> bool = std::mem::transmute(entry);
                if !entry(handle) {
                    return Err(format!("the module entry of {} failed", binary.display()));
                }
                library.entered = true;
            }
            Ok(library)
        }

        pub fn symbol(&self, name: &str) -> *mut c_void {
            let name = CString::new(name).unwrap_or_default();
            // SAFETY: dlsym on a handle this object keeps open.
            unsafe { libc::dlsym(self.handle, name.as_ptr()) }
        }

        pub fn bundle_version(&self) -> String {
            String::new()
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: the handle is open; every object of the module is released by now.
            unsafe {
                if self.entered {
                    let exit = self.symbol("ModuleExit");
                    if !exit.is_null() {
                        let exit: unsafe extern "system" fn() -> bool = std::mem::transmute(exit);
                        exit();
                    }
                }
                libc::dlclose(self.handle);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    //! Through `CFBundle`, as the SDK's own host does: `bundleEntry` wants the bundle
    //! reference, and plug-ins look their resources up through it. Type-checked only.
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    type CFTypeRef = *const c_void;
    type CFIndex = isize;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFTypeRef,
            buffer: *const u8,
            length: CFIndex,
            is_directory: u8,
        ) -> CFTypeRef;
        fn CFBundleCreate(allocator: CFTypeRef, url: CFTypeRef) -> CFTypeRef;
        fn CFBundleLoadExecutable(bundle: CFTypeRef) -> u8;
        fn CFBundleGetFunctionPointerForName(bundle: CFTypeRef, name: CFTypeRef) -> *mut c_void;
        fn CFBundleGetValueForInfoDictionaryKey(bundle: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithCString(allocator: CFTypeRef, string: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFStringGetCString(string: CFTypeRef, buffer: *mut c_char, size: CFIndex, encoding: u32) -> u8;
        fn CFStringGetTypeID() -> usize;
        fn CFGetTypeID(object: CFTypeRef) -> usize;
        fn CFRelease(object: CFTypeRef);
    }

    const UTF8: u32 = 0x0800_0100;

    struct CfString(CFTypeRef);

    impl CfString {
        fn new(text: &str) -> CfString {
            let text = std::ffi::CString::new(text).unwrap_or_default();
            // SAFETY: a NUL-terminated UTF-8 string.
            CfString(unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) })
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: created by this object.
                unsafe { CFRelease(self.0) };
            }
        }
    }

    pub struct Library {
        bundle: CFTypeRef,
        entered: bool,
    }

    impl Library {
        pub unsafe fn open(path: &Path) -> Result<Library, String> {
            let bytes = path.as_os_str().as_bytes();
            let url =
                CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex, 1);
            if url.is_null() {
                return Err(format!("bad VST3 bundle path {}", path.display()));
            }
            let bundle = CFBundleCreate(std::ptr::null(), url);
            CFRelease(url);
            if bundle.is_null() {
                return Err(format!("{} is not a bundle", path.display()));
            }
            let mut library = Library { bundle, entered: false };
            if CFBundleLoadExecutable(bundle) == 0 {
                return Err(format!(
                    "cannot load {} (architecture, a missing dependency, or its code signature)",
                    path.display()
                ));
            }
            let entry = library.symbol("bundleEntry");
            if !entry.is_null() {
                let entry: unsafe extern "system" fn(CFTypeRef) -> bool = std::mem::transmute(entry);
                if !entry(bundle) {
                    return Err(format!("the bundle entry of {} failed", path.display()));
                }
                library.entered = true;
            }
            Ok(library)
        }

        pub fn symbol(&self, name: &str) -> *mut c_void {
            let name = CfString::new(name);
            // SAFETY: the bundle is loaded and kept by this object.
            unsafe { CFBundleGetFunctionPointerForName(self.bundle, name.0) }
        }

        pub fn bundle_version(&self) -> String {
            let key = CfString::new("CFBundleShortVersionString");
            // SAFETY: the returned value is borrowed from the bundle and checked to be a string.
            unsafe {
                let value = CFBundleGetValueForInfoDictionaryKey(self.bundle, key.0);
                if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
                    return String::new();
                }
                let mut buffer = [0 as c_char; 256];
                if CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as CFIndex, UTF8) == 0 {
                    return String::new();
                }
                chars_to_string(&buffer)
            }
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: the bundle is loaded; every object of the module is released by now.
            // The executable is not unloaded explicitly: CoreFoundation does that with the
            // last reference, and Objective-C code can never be unloaded anyway.
            unsafe {
                if self.entered {
                    let exit = self.symbol("bundleExit");
                    if !exit.is_null() {
                        let exit: unsafe extern "system" fn() -> bool = std::mem::transmute(exit);
                        exit();
                    }
                }
                CFRelease(self.bundle);
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    //! Written from the SDK documentation. Type-checked on its own only: the workspace
    //! is Unix only.
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    pub struct Library {
        handle: *mut c_void,
        entered: bool,
    }

    impl Library {
        pub unsafe fn open(path: &Path) -> Result<Library, String> {
            let binary = library_in_bundle(path)?;
            let wide: Vec<u16> = binary.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let handle = LoadLibraryW(wide.as_ptr());
            if handle.is_null() {
                return Err(format!("cannot load {}: {}", binary.display(), std::io::Error::last_os_error()));
            }
            let mut library = Library { handle, entered: false };
            let entry = library.symbol("InitDll");
            if !entry.is_null() {
                let entry: unsafe extern "system" fn() -> bool = std::mem::transmute(entry);
                if !entry() {
                    return Err(format!("InitDll of {} failed", binary.display()));
                }
                library.entered = true;
            }
            Ok(library)
        }

        pub fn symbol(&self, name: &str) -> *mut c_void {
            let name = std::ffi::CString::new(name).unwrap_or_default();
            // SAFETY: the module is loaded and kept by this object.
            unsafe { GetProcAddress(self.handle, name.as_ptr()) }
        }

        pub fn bundle_version(&self) -> String {
            String::new()
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: the module is loaded; every object of it is released by now.
            unsafe {
                if self.entered {
                    let exit = self.symbol("ExitDll");
                    if !exit.is_null() {
                        let exit: unsafe extern "system" fn() -> bool = std::mem::transmute(exit);
                        exit();
                    }
                }
                FreeLibrary(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_print_and_parse_like_the_sdk() {
        let cid = vst3::uid(0x5354_5633, 0x4761_696E, 0x5072_6F63, 0x0000_0001);
        assert_eq!(cid_to_hex(&cid), "535456334761696E50726F6300000001");
        assert_eq!(hex_to_cid("535456334761696e50726f6300000001"), Some(cid));
        assert_eq!(hex_to_cid("{53545633-4761696E-50726F63-00000001}"), Some(cid));
        assert_eq!(hex_to_cid("535456334761696E50726F630000000"), None);
        assert_eq!(hex_to_cid("535456334761696E50726F630000000G"), None);
        assert_eq!(hex_to_cid(""), None);
    }

    #[test]
    fn fixed_strings_stop_at_the_terminator_or_the_array_end() {
        assert_eq!(chars_to_string(&[b'a' as c_char, b'b' as c_char, 0, b'c' as c_char]), "ab");
        assert_eq!(chars_to_string(&[b'a' as c_char, b'b' as c_char]), "ab");
        assert_eq!(wide_to_string(&[0x266A, 0x41, 0, 0x42]), "\u{266A}A");
        assert_eq!(wide_to_string(&[0x41, 0x42]), "AB");
    }
}
