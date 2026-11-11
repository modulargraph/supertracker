//! The window: the interface in a web view, the menu bar, native context menus and file choosers,
//! and the quit handshake.
//!
//! The page talks to the host through the web view's message channel: it posts request texts with
//! `window.ipc.postMessage`, and the host delivers replies and events by calling
//! `STHostTransport.receive` (host-transport.js). Deliveries are batched and acknowledged: while
//! the page has not acknowledged a batch (it is busy, or hung), nothing more is pushed into the web
//! view, and the outbox keeps only the newest of each snapshot event.
//!
//! AI clients get their screenshots from here too: the request arrives as an event of the loop and
//! the web view takes the picture (`screenshot.rs`).
//!
//! Implemented for macOS and Linux (GTK). The macOS-only parts are type-checked from Linux and have
//! not run on a Mac yet; docs/HOST.md lists what was exercised.

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn run(_: &crate::cli::Options) -> Result<(), String> {
    Err("The window is implemented for macOS and Linux. On this system, run with --serve and open the printed address in a browser.".to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use desktop::run;

/// What the window shows before the page has drawn, and behind anything the page leaves
/// transparent: red, green, blue, alpha.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub const BACKGROUND: (u8, u8, u8, u8) = (14, 17, 22, 255);

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod desktop {
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    #[cfg(target_os = "linux")]
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use muda::accelerator::Accelerator;
    use muda::{AboutMetadata, CheckMenuItem, ContextMenu, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use st_host::{install, Bridge, Dialogs, FileDialog, HostOptions, Outbox, QuitProgress, QuitRequest, Reply};
    use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tao::window::{Window, WindowBuilder};
    use wry::http::{header::CONTENT_TYPE, Request, Response};
    use wry::WebViewBuilder;

    use crate::cli::Options;
    use crate::menus::{self, Item, Role, Standard, MENUS};
    use super::BACKGROUND;
    use crate::{native, screenshot, serve};

    /// The interface is loaded from `supertracker://localhost/…`, served from the interface folder.
    const SCHEME: &str = "supertracker";
    const START_URL: &str = "supertracker://localhost/index.html";
    const QUIT_ID: &str = "app:quit";
    const CONTEXT_ID: &str = "context:";
    /// An interface that has not reacted to the quit question after this long is treated as hung.
    const QUIT_PATIENCE: Duration = Duration::from_secs(8);
    /// A delivery that is not acknowledged (the page reloaded) stops holding newer ones back after this.
    const DELIVERY_PATIENCE: Duration = Duration::from_secs(5);

    enum UserEvent {
        /// The outbox has messages for the page.
        Messages,
        /// A menu item was chosen.
        Menu(String),
        ShowContextMenu(ContextMenuPayload, Reply<ContextMenuResult>),
        /// The context menu with this serial closed; a choice, if any, was posted before this.
        ContextMenuClosed(u64),
        #[cfg(target_os = "linux")]
        ChooseFile(FileDialog, mpsc::Sender<Option<PathBuf>>),
        Quit(QuitProgress),
        #[cfg(target_os = "macos")]
        TitleBar { double_click: bool },
        /// A plug-in container (by process id) is about to show an editor window and must be
        /// allowed to take the keyboard; the sender is answered once AppKit has been told.
        #[cfg(target_os = "macos")]
        YieldActivation(u32, std::sync::mpsc::Sender<()>),
        /// An AI client asked for a picture of the window; the request carries where the answer goes.
        Screenshot(screenshot::Request),
    }

    /// Posts events to the window's loop from any thread.
    struct Poster(Mutex<EventLoopProxy<UserEvent>>);

    impl Poster {
        /// False when the loop has ended.
        fn post(&self, event: UserEvent) -> bool {
            self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send_event(event).is_ok()
        }
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct ContextMenuPayload {
        items: Vec<ContextItem>,
        x: f64,
        y: f64,
    }

    /// `{title, id?, enabled?, checked?, key?, items?}`, `{separator: true}` or `{header: true, title}`.
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct ContextItem {
        id: Option<String>,
        title: String,
        enabled: Option<bool>,
        checked: bool,
        separator: bool,
        header: bool,
        /// A shortcut to display, in the interface's notation ("⇧⌘Z").
        key: Option<String>,
        items: Vec<ContextItem>,
    }

    #[derive(Debug, Serialize)]
    struct ContextMenuResult {
        id: Option<String>,
    }

    /// `window.titleBar`: the pointer went down on empty title bar space of the page.
    #[cfg(target_os = "macos")]
    #[derive(Debug, Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct TitleBarPayload {
        double_click: bool,
    }

    /// The `window` section of the settings.
    #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct WindowSettings {
        /// Logical size of the content.
        width: f64,
        height: f64,
        /// Physical position of the frame; absent until the window has been moved.
        x: Option<i32>,
        y: Option<i32>,
        maximized: bool,
    }

    impl Default for WindowSettings {
        fn default() -> Self {
            WindowSettings { width: 1500.0, height: 940.0, x: None, y: None, maximized: false }
        }
    }

    const MIN_SIZE: LogicalSize<f64> = LogicalSize { width: 1080.0, height: 700.0 };

    /// File choosers shown by the platform.
    struct NativeDialogs {
        #[cfg_attr(target_os = "macos", allow(dead_code))]
        poster: Arc<Poster>,
    }

    impl Dialogs for NativeDialogs {
        #[cfg(target_os = "macos")]
        fn choose(&self, dialog: &FileDialog) -> st_host::Result<Option<PathBuf>> {
            Ok(native::choose_file(dialog))
        }

        #[cfg(target_os = "linux")]
        fn choose(&self, dialog: &FileDialog) -> st_host::Result<Option<PathBuf>> {
            let (answer, chosen) = mpsc::channel();
            if !self.poster.post(UserEvent::ChooseFile(dialog.clone(), answer)) {
                return Err(st_host::Error::new("The window is closing."));
            }
            chosen.recv().map_err(|_| st_host::Error::new("The window closed before a file was chosen."))
        }
    }

    /// A context menu that is open, and what its items mean to the page.
    struct OpenContextMenu {
        serial: u64,
        reply: Reply<ContextMenuResult>,
        /// Menu id → the interface's item id.
        ids: HashMap<String, String>,
        /// Keeps the native menu alive while it is shown.
        _menu: Menu,
    }

    pub fn run(options: &Options) -> Result<(), String> {
        let interface = crate::ui::locate(options.ui.as_deref())?;
        let support = st_host::support_dir().map_err(|error| error.to_string())?;

        let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
        let poster = Arc::new(Poster(Mutex::new(event_loop.create_proxy())));
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
            event_loop.set_activation_policy(ActivationPolicy::Regular);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = &mut event_loop;

        // The host and the commands only a window can serve.
        let dialogs: Arc<dyn Dialogs> = crate::scripted_dialogs(options).unwrap_or_else(|| Arc::new(NativeDialogs { poster: poster.clone() }));
        let mut bridge = Bridge::builder();
        // On the main thread: the MIDI service is created here (CoreMIDI wants that).
        let host = Arc::new(install(&mut bridge, HostOptions::new(support, dialogs)).map_err(|error| error.to_string())?);
        crate::watch_signals(&host);
        {
            let poster = poster.clone();
            bridge.commands().register_deferred("ui.contextMenu", move |payload: ContextMenuPayload, reply: Reply<ContextMenuResult>| {
                // A loop that has ended drops the event, and with it the reply, which then answers with an error.
                poster.post(UserEvent::ShowContextMenu(payload, reply));
            });
        }
        #[cfg(target_os = "macos")]
        {
            let poster = poster.clone();
            bridge.commands().register("window.titleBar", move |payload: TitleBarPayload| -> st_host::Result<()> {
                poster.post(UserEvent::TitleBar { double_click: payload.double_click });
                Ok(())
            });
        }
        #[cfg(target_os = "macos")]
        {
            // Plug-in editors are windows of a container process. Since macOS 14 this application
            // has to yield activation to that process before the window can become key. The
            // request comes from a bridge thread and waits (briefly) for the main thread.
            let poster = poster.clone();
            st_host::areas::plugins::set_activation_yielder(Box::new(move |container_pid| {
                let (done, wait) = std::sync::mpsc::channel();
                if poster.post(UserEvent::YieldActivation(container_pid, done)) {
                    let _ = wait.recv_timeout(Duration::from_millis(500));
                }
            }));
        }
        if let Some(agent) = &host.services.agent {
            // AI clients ask for screenshots on a thread of the control server; the web view takes
            // them on this one. A loop that is busy for too long costs the client an error.
            let poster = poster.clone();
            agent.set_screenshot_provider(screenshot::provider(move |request| poster.post(UserEvent::Screenshot(request)), screenshot::PATIENCE));
        }
        if let Some(project) = &options.open {
            host.session.request_open(std::path::absolute(project).unwrap_or_else(|_| project.clone()));
        }
        {
            let poster = poster.clone();
            host.lifecycle.on_progress(move |progress| {
                poster.post(UserEvent::Quit(progress));
            });
        }
        let bridge = bridge.start();
        let events = bridge.events();

        // The window.
        let stored: WindowSettings = host.settings.get("window");
        let mut builder = WindowBuilder::new()
            .with_title("SuperTracker")
            .with_inner_size(LogicalSize::new(stored.width.max(MIN_SIZE.width), stored.height.max(MIN_SIZE.height)))
            .with_min_inner_size(MIN_SIZE)
            .with_maximized(stored.maximized)
            .with_background_color(BACKGROUND);
        if let (Some(x), Some(y)) = (stored.x, stored.y) {
            builder = builder.with_position(PhysicalPosition::new(x, y));
        }
        #[cfg(target_os = "macos")]
        {
            // The page draws under a transparent title bar and moves the window itself (`window.titleBar`).
            use tao::platform::macos::WindowBuilderExtMacOS;
            builder = builder.with_titlebar_transparent(true).with_fullsize_content_view(true).with_title_hidden(true);
        }
        let window = builder.build(&event_loop).map_err(|error| format!("The window could not be created: {error}."))?;

        // The menu bar.
        let menu_bar = build_menu_bar()?;
        #[cfg(target_os = "macos")]
        menu_bar.init_for_nsapp();
        #[cfg(target_os = "linux")]
        {
            use tao::platform::unix::WindowExtUnix;
            menu_bar
                .init_for_gtk_window(window.gtk_window(), window.default_vbox())
                .map_err(|error| format!("The menu bar could not be created: {error}."))?;
        }
        {
            let poster = poster.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                poster.post(UserEvent::Menu(event.id.0));
            }));
        }

        // The web view and the bridge's connection to it.
        let outbox = {
            let poster = poster.clone();
            Outbox::with_wake(move || {
                poster.post(UserEvent::Messages);
            })
        };
        let connection = Rc::new(bridge.connect(outbox.clone()));
        let acknowledged = Rc::new(std::cell::Cell::new(false));
        let files = interface.clone();
        let web_view_builder = WebViewBuilder::new()
            .with_custom_protocol(SCHEME.to_string(), move |_, request| serve_file(&files, &request))
            .with_url(START_URL)
            .with_ipc_handler({
                let (connection, acknowledged, poster) = (connection.clone(), acknowledged.clone(), poster.clone());
                move |message: Request<String>| {
                    let text = message.body();
                    if text.is_empty() {
                        // The page finished handling a delivery (host-transport.js).
                        acknowledged.set(true);
                        poster.post(UserEvent::Messages);
                    } else {
                        connection.receive(text);
                    }
                }
            })
            // The page never navigates away; links open in the browser through `app.external`.
            .with_navigation_handler(|url| url.starts_with("supertracker:") || url == "about:blank")
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_back_forward_navigation_gestures(false)
            .with_hotkeys_zoom(false)
            .with_accept_first_mouse(true)
            .with_background_color(BACKGROUND)
            .with_devtools(true);
        #[cfg(target_os = "macos")]
        let web_view = web_view_builder.build(&window);
        #[cfg(target_os = "linux")]
        let web_view = {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            let container = window.default_vbox().ok_or_else(|| "The window has no place for the interface.".to_string())?;
            web_view_builder.build_gtk(container)
        };
        let web_view = web_view.map_err(|error| format!("The interface could not be shown: {error}."))?;
        // The interface is attached: AI control may listen.
        host.services.start_agent();

        let mut in_flight: Option<Instant> = None;
        let mut quit_deadline: Option<Instant> = None;
        let mut context_menu: Option<OpenContextMenu> = None;
        let mut next_context_serial = 0u64;
        let mut frame = stored;
        let mut finished = false;

        event_loop.run(move |event, _, control_flow| {
            // Wake for whichever deadline comes first; otherwise sleep until an event.
            let mut finish = false;
            match event {
                Event::UserEvent(UserEvent::Messages) => {}
                Event::UserEvent(UserEvent::Menu(id)) => {
                    if id == QUIT_ID {
                        begin_quit(&host.lifecycle, &mut quit_deadline, &mut finish);
                    } else if let Some(name) = menus::command_for_id(&id) {
                        events.emit("command", json!({ "name": name }));
                    } else if id.starts_with(CONTEXT_ID) {
                        if let Some(open) = context_menu.take() {
                            let chosen = open.ids.get(&id).cloned();
                            open.reply.ok(ContextMenuResult { id: chosen });
                        }
                    }
                }
                Event::UserEvent(UserEvent::ShowContextMenu(payload, reply)) => {
                    if context_menu.is_some() {
                        reply.error("Another menu is still open.");
                    } else {
                        next_context_serial += 1;
                        let serial = next_context_serial;
                        match show_context_menu(&window, &payload, serial, &poster) {
                            Ok((menu, ids)) => context_menu = Some(OpenContextMenu { serial, reply, ids, _menu: menu }),
                            Err(message) => reply.error(message),
                        }
                    }
                }
                Event::UserEvent(UserEvent::ContextMenuClosed(serial)) => {
                    if context_menu.as_ref().is_some_and(|open| open.serial == serial) {
                        if let Some(open) = context_menu.take() {
                            open.reply.ok(ContextMenuResult { id: None });
                        }
                    }
                }
                #[cfg(target_os = "linux")]
                Event::UserEvent(UserEvent::ChooseFile(dialog, answer)) => {
                    use tao::platform::unix::WindowExtUnix;
                    use gtk::prelude::Cast;
                    let _ = answer.send(native::choose_file(&dialog, window.gtk_window().upcast_ref()));
                }
                Event::UserEvent(UserEvent::Quit(progress)) => match progress {
                    QuitProgress::Deciding | QuitProgress::Stay(_) => quit_deadline = None,
                    QuitProgress::Proceed => finish = true,
                },
                #[cfg(target_os = "macos")]
                Event::UserEvent(UserEvent::TitleBar { double_click }) => {
                    if double_click {
                        window.set_maximized(!window.is_maximized());
                    } else {
                        let _ = window.drag_window();
                    }
                }
                #[cfg(target_os = "macos")]
                Event::UserEvent(UserEvent::YieldActivation(container_pid, done)) => {
                    crate::native::yield_activation_to(container_pid);
                    let _ = done.send(());
                }
                #[cfg(target_os = "linux")]
                Event::UserEvent(UserEvent::Screenshot(request)) => screenshot::take(&web_view, request),
                #[cfg(target_os = "macos")]
                Event::UserEvent(UserEvent::Screenshot(request)) => screenshot::take(&web_view, window.scale_factor(), request),
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => begin_quit(&host.lifecycle, &mut quit_deadline, &mut finish),
                Event::WindowEvent { event: WindowEvent::Resized(size), .. } => remember_size(&window, size, &mut frame),
                Event::WindowEvent { event: WindowEvent::Moved(position), .. } => {
                    if !window.is_maximized() && !window.is_minimized() {
                        (frame.x, frame.y) = (Some(position.x), Some(position.y));
                    }
                }
                Event::Opened { urls } => {
                    // The system asks to open project files (Finder, the Dock icon).
                    let project = urls.iter().filter_map(|url| url.to_file_path().ok()).rfind(|path| is_project_file(path));
                    if let Some(path) = project {
                        host.session.request_open(path);
                        window.set_minimized(false);
                        window.set_focus();
                    }
                }
                Event::LoopDestroyed => {
                    // The loop ended without the handshake (the system is shutting down): keep what the host has.
                    if !finished {
                        let _ = host.session.save_recovery();
                        host.services.shutdown();
                        bridge.shutdown(Duration::from_millis(500));
                    }
                    return;
                }
                _ => {}
            }

            // A quit question nobody answered: the interface is hung.
            if !finish && quit_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                quit_deadline = None;
                if host.lifecycle.is_unanswered() {
                    let (title, detail) = (
                        "SuperTracker's interface is not responding",
                        "Your latest edits could not be collected. The last automatic recovery copy is kept either way.",
                    );
                    #[cfg(target_os = "macos")]
                    let quit_anyway = native::confirm(title, detail, "Quit Anyway", "Keep Working");
                    #[cfg(target_os = "linux")]
                    let quit_anyway = {
                        use tao::platform::unix::WindowExtUnix;
                        use gtk::prelude::Cast;
                        native::confirm(title, detail, "Quit Anyway", "Keep Working", window.gtk_window().upcast_ref())
                    };
                    if quit_anyway {
                        finish = true;
                    } else {
                        host.lifecycle.give_up();
                    }
                }
            }

            if finish && !finished {
                finished = true;
                frame.maximized = window.is_maximized();
                if let Err(error) = host.settings.set("window", &frame) {
                    eprintln!("supertracker: the window's place could not be remembered: {error}");
                }
                host.session.stop();
                if let Err(error) = host.session.save_recovery() {
                    eprintln!("supertracker: the recovery copy could not be written: {error}");
                }
                host.services.shutdown();
                bridge.shutdown(Duration::from_secs(2));
                *control_flow = ControlFlow::Exit;
                return;
            }

            // Deliver what waits for the page, one acknowledged batch at a time.
            if acknowledged.replace(false) {
                in_flight = None;
            }
            if in_flight.is_some_and(|since| since.elapsed() >= DELIVERY_PATIENCE) {
                in_flight = None;
            }
            if in_flight.is_none() && !outbox.is_empty() {
                let script = delivery_script(&outbox.drain());
                match web_view.evaluate_script(&script) {
                    Ok(()) => in_flight = Some(Instant::now()),
                    Err(error) => eprintln!("supertracker: a message could not be delivered to the interface: {error}"),
                }
            }

            let delivery_deadline = in_flight.filter(|_| !outbox.is_empty()).map(|since| since + DELIVERY_PATIENCE);
            *control_flow = match [quit_deadline, delivery_deadline].into_iter().flatten().min() {
                Some(deadline) => ControlFlow::WaitUntil(deadline),
                None => ControlFlow::Wait,
            };
        });
    }

    /// The window was asked to close, or Quit was chosen.
    fn begin_quit(lifecycle: &st_host::Lifecycle, quit_deadline: &mut Option<Instant>, finish: &mut bool) {
        match lifecycle.request_quit() {
            QuitRequest::QuitNow => *finish = true,
            QuitRequest::Asked => *quit_deadline = Some(Instant::now() + QUIT_PATIENCE),
            QuitRequest::AlreadyAsked => {}
        }
    }

    fn remember_size(window: &Window, size: PhysicalSize<u32>, frame: &mut WindowSettings) {
        if window.is_maximized() || window.is_minimized() || size.width == 0 {
            return;
        }
        let logical: LogicalSize<f64> = size.to_logical(window.scale_factor());
        (frame.width, frame.height) = (logical.width, logical.height);
    }

    fn is_project_file(path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("supertracker"))
    }

    /// The script that hands a batch of messages to the page and acknowledges it (an empty post).
    /// Each message is a JSON text, which is also a JavaScript expression.
    fn delivery_script(messages: &[String]) -> String {
        let mut script = String::from("(function(t){try{if(t){");
        for message in messages {
            script.push_str("t.receive(");
            script.push_str(message);
            script.push_str(");");
        }
        script.push_str("}}finally{window.ipc.postMessage('')}})(window.STHostTransport)");
        script
    }

    /// Answers a `supertracker://localhost/<file>` request from the interface folder.
    fn serve_file(root: &Path, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
        let respond = |status: u16, content_type: &str, body: Vec<u8>| {
            Response::builder()
                .status(status)
                .header(CONTENT_TYPE, content_type)
                .header("Cache-Control", "no-store")
                .body(Cow::Owned(body))
                .unwrap_or_else(|_| Response::new(Cow::Borrowed(&b""[..])))
        };
        match serve::resolve(root, request.uri().path()).and_then(|file| std::fs::read(&file).ok().map(|body| (file, body))) {
            Some((file, body)) => respond(200, serve::content_type(&file), body),
            None => respond(404, "text/plain; charset=utf-8", b"Not found.".to_vec()),
        }
    }

    fn menu_error(error: muda::Error) -> String {
        format!("The menus could not be created: {error}.")
    }

    fn build_menu_bar() -> Result<Menu, String> {
        let bar = Menu::new();
        for (menu_index, spec) in MENUS.iter().enumerate() {
            // Only macOS has an application menu; elsewhere its commands move to File.
            if spec.role == Role::Application && !cfg!(target_os = "macos") {
                continue;
            }
            // A menu whose items all belong to another platform (Window, outside macOS) is left out.
            if !cfg!(target_os = "macos") && spec.items.iter().all(|item| !matches!(item, Item::Command { .. })) {
                continue;
            }
            let submenu = Submenu::new(spec.title, true);
            for (line, item) in spec.items.iter().enumerate() {
                append_item(&submenu, menu_index, line, item)?;
            }
            if spec.title == "File" && !cfg!(target_os = "macos") {
                let application = MENUS.iter().position(|menu| menu.role == Role::Application);
                if let Some(index) = application {
                    submenu.append(&PredefinedMenuItem::separator()).map_err(menu_error)?;
                    for (line, item) in MENUS[index].items.iter().enumerate() {
                        if matches!(item, Item::Command { .. } | Item::Standard(Standard::About | Standard::Quit)) {
                            append_item(&submenu, index, line, item)?;
                        }
                    }
                }
            }
            bar.append(&submenu).map_err(menu_error)?;
            #[cfg(target_os = "macos")]
            match spec.role {
                Role::Window => submenu.set_as_windows_menu_for_nsapp(),
                Role::Help => submenu.set_as_help_menu_for_nsapp(),
                _ => {}
            }
        }
        Ok(bar)
    }

    fn append_item(menu: &Submenu, menu_index: usize, line: usize, item: &Item) -> Result<(), String> {
        let mac = cfg!(target_os = "macos");
        let built: Option<Box<dyn IsMenuItem>> = match *item {
            Item::Separator => Some(Box::new(PredefinedMenuItem::separator())),
            Item::Command { title, shortcut, .. } => {
                let accelerator = shortcut.and_then(|shortcut| shortcut.parse::<Accelerator>().ok());
                Some(Box::new(MenuItem::with_id(menus::id_for(menu_index, line), title, true, accelerator)))
            }
            Item::Standard(standard) => match standard {
                Standard::About => {
                    let metadata = AboutMetadata {
                        name: Some("SuperTracker".to_string()),
                        version: Some(env!("CARGO_PKG_VERSION").to_string()),
                        comments: Some("Keyboard-first tracker studio.".to_string()),
                        ..AboutMetadata::default()
                    };
                    Some(Box::new(PredefinedMenuItem::about(Some("About SuperTracker"), Some(metadata))))
                }
                // Quit goes through the handshake with the interface, so it is the host's own item.
                Standard::Quit => Some(Box::new(MenuItem::with_id(QUIT_ID, "Quit SuperTracker", true, "CmdOrCtrl+Q".parse::<Accelerator>().ok()))),
                Standard::CloseWindow => Some(Box::new(PredefinedMenuItem::close_window(Some("Close Window")))),
                Standard::Cut => Some(Box::new(PredefinedMenuItem::cut(None))),
                Standard::Copy => Some(Box::new(PredefinedMenuItem::copy(None))),
                Standard::Paste => Some(Box::new(PredefinedMenuItem::paste(None))),
                Standard::Hide if mac => Some(Box::new(PredefinedMenuItem::hide(Some("Hide SuperTracker")))),
                Standard::HideOthers if mac => Some(Box::new(PredefinedMenuItem::hide_others(None))),
                Standard::ShowAll if mac => Some(Box::new(PredefinedMenuItem::show_all(None))),
                Standard::Minimize if mac => Some(Box::new(PredefinedMenuItem::minimize(None))),
                Standard::Zoom if mac => Some(Box::new(PredefinedMenuItem::zoom(Some("Zoom")))),
                Standard::BringAllToFront if mac => Some(Box::new(PredefinedMenuItem::bring_all_to_front(None))),
                Standard::Fullscreen if mac => Some(Box::new(PredefinedMenuItem::fullscreen(Some("Enter Full Screen")))),
                // GTK 3 has no native form of these.
                _ => None,
            },
        };
        match built {
            Some(item) => menu.append(item.as_ref()).map_err(menu_error),
            None => Ok(()),
        }
    }

    /// Builds the native menu for `ui.contextMenu` and shows it at the page's coordinates. Returns
    /// the menu (its items must outlive the choice's menu event) and the meaning of its ids. When the
    /// menu has closed, `ContextMenuClosed(serial)` is posted, after the choice if there was one.
    fn show_context_menu(window: &Window, payload: &ContextMenuPayload, serial: u64, poster: &Arc<Poster>) -> Result<(Menu, HashMap<String, String>), String> {
        let menu = Menu::new();
        let mut ids = HashMap::new();
        for item in payload.items.iter().take(200) {
            let built = context_item(item, serial, &mut ids)?;
            menu.append(built.as_ref()).map_err(menu_error)?;
        }
        let position = muda::dpi::Position::Logical(muda::dpi::LogicalPosition::new(payload.x, payload.y));
        // Both calls block until the menu closes (the platform runs its own loop meanwhile). A choice
        // reaches the loop as a menu event, posted before the "closed" that follows here.
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::WindowExtMacOS;
            // SAFETY: the pointer is this window's content view, alive for the duration of the call.
            unsafe { menu.show_context_menu_for_nsview(window.ns_view() as _, Some(position)) };
        }
        #[cfg(target_os = "linux")]
        {
            use gtk::prelude::Cast;
            use tao::platform::unix::WindowExtUnix;
            let window: &gtk::Window = window.gtk_window().upcast_ref();
            menu.show_context_menu_for_gtk_window(window, Some(position));
        }
        poster.post(UserEvent::ContextMenuClosed(serial));
        Ok((menu, ids))
    }

    fn context_item(item: &ContextItem, serial: u64, ids: &mut HashMap<String, String>) -> Result<Box<dyn IsMenuItem>, String> {
        if item.separator {
            return Ok(Box::new(PredefinedMenuItem::separator()));
        }
        if item.header {
            return Ok(Box::new(MenuItem::new(&item.title, false, None)));
        }
        let enabled = item.enabled.unwrap_or(true);
        if !item.items.is_empty() {
            let submenu = Submenu::new(&item.title, enabled);
            for child in item.items.iter().take(200) {
                submenu.append(context_item(child, serial, ids)?.as_ref()).map_err(menu_error)?;
            }
            return Ok(Box::new(submenu));
        }
        // Standard editing is the platform's own, exactly like the Edit menu.
        match item.id.as_deref() {
            Some("native:cut") => return Ok(Box::new(PredefinedMenuItem::cut(Some(&item.title)))),
            Some("native:copy") => return Ok(Box::new(PredefinedMenuItem::copy(Some(&item.title)))),
            Some("native:paste") => return Ok(Box::new(PredefinedMenuItem::paste(Some(&item.title)))),
            Some("native:selectAll") => return Ok(Box::new(PredefinedMenuItem::select_all(Some(&item.title)))),
            _ => {}
        }
        let menu_id = format!("{CONTEXT_ID}{serial}/{}", ids.len());
        if let Some(id) = &item.id {
            ids.insert(menu_id.clone(), id.clone());
        }
        // Shortcut hints are display-only; only AppKit shows one without also claiming the key.
        let hint = if cfg!(target_os = "macos") { item.key.as_deref().and_then(shortcut_hint) } else { None };
        if item.checked {
            Ok(Box::new(CheckMenuItem::with_id(menu_id, &item.title, enabled, true, hint)))
        } else {
            Ok(Box::new(MenuItem::with_id(menu_id, &item.title, enabled, hint)))
        }
    }

    /// "⇧⌘Z" in the interface's notation as an accelerator, when it is a plain key with modifiers.
    fn shortcut_hint(text: &str) -> Option<Accelerator> {
        let mut parts: Vec<&str> = Vec::new();
        let mut rest = text;
        loop {
            let (modifier, length) = match rest.chars().next()? {
                '⌘' => ("Cmd", '⌘'.len_utf8()),
                '⌥' => ("Alt", '⌥'.len_utf8()),
                '⇧' => ("Shift", '⇧'.len_utf8()),
                '⌃' => ("Ctrl", '⌃'.len_utf8()),
                _ => break,
            };
            parts.push(modifier);
            rest = &rest[length..];
        }
        let key = match rest {
            "⌫" => "Backspace",
            "⌦" => "Delete",
            "↩" => "Enter",
            "Esc" => "Escape",
            other if other.chars().count() == 1 || matches!(other, "Space" | "Tab") => other,
            _ => return None,
        };
        parts.push(key);
        parts.join("+").parse().ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_delivery_hands_over_every_message_and_always_acknowledges() {
            assert_eq!(
                delivery_script(&[r#"{"id":1,"ok":true,"result":null}"#.to_string(), r#"{"type":"status","payload":{}}"#.to_string()]),
                r#"(function(t){try{if(t){t.receive({"id":1,"ok":true,"result":null});t.receive({"type":"status","payload":{}});}}finally{window.ipc.postMessage('')}})(window.STHostTransport)"#
            );
        }

        #[test]
        fn window_settings_default_to_the_design_size_and_tolerate_old_files() {
            let defaults = WindowSettings::default();
            assert_eq!((defaults.width, defaults.height, defaults.maximized), (1500.0, 940.0, false));
            let partial: WindowSettings = serde_json::from_str(r#"{"width": 1200, "future": true}"#).unwrap();
            assert_eq!((partial.width, partial.height, partial.x), (1200.0, 940.0, None));
        }

        #[test]
        fn only_project_files_are_opened_from_the_system() {
            assert!(is_project_file(Path::new("/music/Song.supertracker")));
            assert!(is_project_file(Path::new("/music/SONG.SUPERTRACKER")));
            assert!(!is_project_file(Path::new("/music/Song.json")));
            assert!(!is_project_file(Path::new("/music/supertracker")));
        }

        #[test]
        fn context_menu_payloads_are_read_as_the_page_sends_them() {
            let payload: ContextMenuPayload = serde_json::from_value(json!({
                "x": 120, "y": 48.5,
                "items": [
                    {"title": "Cut", "enabled": true, "id": "native:cut", "key": "⌘X"},
                    {"separator": true},
                    {"header": true, "title": "Track"},
                    {"title": "Mute", "enabled": false, "checked": true, "id": "a0"},
                    {"title": "Move", "enabled": true, "items": [{"title": "Left", "enabled": true, "id": "a1"}]}
                ]
            }))
            .unwrap();
            assert_eq!((payload.x, payload.y, payload.items.len()), (120.0, 48.5, 5));
            assert!(payload.items[1].separator && payload.items[2].header && payload.items[3].checked);
            assert_eq!(payload.items[4].items[0].id.as_deref(), Some("a1"));
        }
    }
}
