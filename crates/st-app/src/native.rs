//! The platform's own panels: file choosers and the one alert the host shows itself.
//!
//! - macOS: `rfd` (NSOpenPanel, NSSavePanel, NSAlert). It moves the call to the main thread itself.
//! - Linux: GTK 3's native file chooser and message dialog. GTK belongs to the window's thread, so
//!   a worker thread asks the event loop to show the panel and waits for the answer.
//!
//! The macOS half is type-checked from Linux (`cargo check --target aarch64-apple-darwin`) but has
//! not run on a Mac yet.

use std::path::PathBuf;

use st_host::{DialogKind, FileDialog};

/// Shows a file chooser. Call on the main thread on Linux; on any thread on macOS.
#[cfg(target_os = "macos")]
pub fn choose_file(dialog: &FileDialog) -> Option<PathBuf> {
    let mut panel = rfd::FileDialog::new().set_title(&dialog.title);
    for filter in &dialog.filters {
        panel = panel.add_filter(&filter.name, &filter.extensions);
    }
    if let Some(name) = &dialog.suggested_name {
        panel = panel.set_file_name(name);
    }
    match dialog.kind {
        DialogKind::OpenFile => panel.pick_file(),
        DialogKind::SaveFile => panel.save_file(),
        DialogKind::ChooseFolder => panel.set_can_create_directories(true).pick_folder(),
    }
}

/// Asks a question with two answers; true for `confirm`.
#[cfg(target_os = "macos")]
pub fn confirm(title: &str, detail: &str, confirm: &str, cancel: &str) -> bool {
    let answer = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title(title)
        .set_description(detail)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(confirm.to_string(), cancel.to_string()))
        .show();
    match answer {
        rfd::MessageDialogResult::Custom(label) => label == confirm,
        rfd::MessageDialogResult::Ok | rfd::MessageDialogResult::Yes => true,
        _ => false,
    }
}

/// Lets the process `pid` (a plug-in container about to show an editor) become the active
/// application: `-[NSApplication yieldActivationToApplication:]`, macOS 14 and later. Without it
/// the container's `activate` request is ignored while this application is active, and the editor
/// opens behind the main window without keyboard focus. Main thread only.
#[cfg(target_os = "macos")]
pub fn yield_activation_to(pid: u32) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel};

    let Ok(pid) = i32::try_from(pid) else { return };
    // SAFETY: plain message sends to AppKit singletons on the main thread; both objects are
    // checked for nil, and the selector is checked before it is sent.
    unsafe {
        let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let container: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
        if application.is_null() || container.is_null() {
            return;
        }
        let supported: bool = msg_send![application, respondsToSelector: sel!(yieldActivationToApplication:)];
        if supported {
            let _: () = msg_send![application, yieldActivationToApplication: container];
        }
    }
}

/// Shows a file chooser. Call on the main thread on Linux; on any thread on macOS.
#[cfg(target_os = "linux")]
pub fn choose_file(dialog: &FileDialog, parent: &gtk::Window) -> Option<PathBuf> {
    use gtk::prelude::*;

    let (action, accept) = match dialog.kind {
        DialogKind::OpenFile => (gtk::FileChooserAction::Open, "Open"),
        DialogKind::SaveFile => (gtk::FileChooserAction::Save, "Save"),
        DialogKind::ChooseFolder => (gtk::FileChooserAction::SelectFolder, "Choose"),
    };
    let accept = dialog.confirm_label.as_deref().unwrap_or(accept);
    let chooser = gtk::FileChooserNative::new(Some(dialog.title.as_str()), Some(parent), action, Some(accept), Some("Cancel"));
    for filter in &dialog.filters {
        let pattern = gtk::FileFilter::new();
        pattern.set_name(Some(filter.name.as_str()));
        for extension in &filter.extensions {
            pattern.add_pattern(&format!("*.{extension}"));
        }
        chooser.add_filter(pattern);
    }
    if dialog.kind == DialogKind::SaveFile {
        chooser.set_do_overwrite_confirmation(true);
        if let Some(name) = &dialog.suggested_name {
            chooser.set_current_name(name);
        }
    }
    let chosen = (chooser.run() == gtk::ResponseType::Accept).then(|| chooser.filename()).flatten();
    chooser.destroy();
    chosen
}

/// Asks a question with two answers; true for `confirm`.
#[cfg(target_os = "linux")]
pub fn confirm(title: &str, detail: &str, confirm: &str, cancel: &str, parent: &gtk::Window) -> bool {
    use gtk::prelude::*;

    let alert = gtk::MessageDialog::new(Some(parent), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::None, title);
    alert.set_secondary_text(Some(detail));
    alert.add_button(cancel, gtk::ResponseType::Cancel);
    alert.add_button(confirm, gtk::ResponseType::Accept);
    let confirmed = alert.run() == gtk::ResponseType::Accept;
    alert.close();
    confirmed
}
