#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    ffi::c_void,
    rc::{Rc, Weak},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use anmixiu_core::GlobalElementId;
use anmixiu_core::{
    AppEvents, AppHandle, AppStateStore, CursorStyle, ErasedComponentHost, Eventful, Pixels,
    PropertyUpdate, Render, SharedString, Typography, Window, WindowAction, WindowDispatcher,
    WindowError, WindowHandle, WindowId, WindowInfo, WindowMode, WindowMountContext, WindowRoot,
    WindowSize, WindowStateStore, WindowStatus, WindowUpdate, WindowVisibility,
};
use anmixiu_reactive::{OwnerId, OwnerRegistry};
use anmixiu_render_metal::{FrameOutcome, MetalRenderer, RenderError, SurfaceSize};
use anmixiu_runtime::{AppRuntime, RuntimeBuildError};
use anmixiu_scene::{Point, Size};
use anmixiu_text_coretext::FontSpec;
use metal::MetalLayer;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSCursor, NSEvent, NSEventModifierFlags, NSEventPhase, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSViewLayerContentsRedrawPolicy, NSWindow,
    NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString,
};
use objc2_quartz_core::CADisplayLink;
use thiserror::Error;

use crate::{BuiltFrame, FrameBuildError, FrameBuilder, PointerTracker, Viewport};

#[cfg(feature = "devtools")]
use crate::{DevToolsAgent, devtools::DevToolsCommand};

const MAX_RENDER_INVALIDATIONS: usize = 8;

// A component driving a continuous animation is legitimate, but one that has been requesting frames
// for this many consecutive turns (~10s at 60fps) is very likely a `request_animation_frame` that
// forgot to stop. We only warn (once per crossing) in debug builds — animation frames are paced by
// the display link, so this wastes work and battery but never pegs a core.
#[cfg(debug_assertions)]
const RUNAWAY_ANIMATION_WARN_FRAMES: u64 = 600;

static MAIN_WAKE_QUEUED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static ACTIVE_APP: RefCell<Option<Rc<AppSession>>> = const { RefCell::new(None) };
}

// SAFETY: These signatures are the stable libdispatch C ABI. The callback is non-capturing,
// uses a null context, and libdispatch invokes it on the process main queue.
unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

extern "C" fn deliver_main_wake(_context: *mut c_void) {
    MAIN_WAKE_QUEUED.store(false, Ordering::Release);
    ACTIVE_APP.with(|active| {
        if let Some(session) = active.borrow().as_ref() {
            session.wake();
        }
    });
}

fn wake_appkit() {
    if MAIN_WAKE_QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: The main dispatch queue is process-global and outlives this call. The callback has
    // the required C ABI and does not dereference its null context.
    unsafe {
        dispatch_async_f(
            std::ptr::addr_of!(_dispatch_main_q).cast_mut(),
            std::ptr::null_mut(),
            deliver_main_wake,
        );
    }
}

fn request_display(window_id: WindowId) {
    ACTIVE_APP.with(|active| {
        if let Some(session) = active.borrow().as_ref() {
            session.request_display(window_id);
        }
    });
}

fn terminate_application() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).terminate(None);
    }
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Anmixiu applications must start on the AppKit main thread")]
    NotMainThread,
    #[error(transparent)]
    Runtime(#[from] RuntimeBuildError),
    #[error(transparent)]
    Frame(#[from] FrameBuildError),
    #[error(transparent)]
    Metal(#[from] RenderError),
    #[error("this Mac does not expose a Metal device")]
    MetalUnavailable,
    #[error("component invalidated itself for more than {0} consecutive display turns")]
    RenderLoop(usize),
    #[error("UI executor thread-affinity failure: {0}")]
    UiThread(String),
    #[error("component render failed: {0}")]
    Component(String),
    #[error(transparent)]
    Window(#[from] WindowError),
}

pub struct App {
    name: SharedString,
    state: AppStateStore,
    window: Window,
    typography: Typography,
    events: AppEvents,
}

impl Default for App {
    fn default() -> Self {
        Self {
            name: SharedString::new_static("Anmixiu"),
            state: AppStateStore::new(),
            window: Window::new(),
            typography: Typography::new(),
            events: AppEvents::new(),
        }
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_state<T: 'static>(mut self, state: T) -> Self {
        self.state = self.state.with(state);
        self
    }

    /// Sets the application name inherited by windows without an explicit title.
    #[must_use]
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
        self
    }

    #[must_use]
    pub fn window(mut self, window: Window) -> Self {
        self.window = window;
        self
    }

    #[must_use]
    pub fn font_family(mut self, family: impl Into<anmixiu_core::SharedString>) -> Self {
        self.typography = self.typography.with_font_family(family);
        self
    }

    #[must_use]
    pub fn font_size(mut self, size: impl Into<Pixels>) -> Self {
        self.typography = self.typography.with_font_size(size);
        self
    }

    /// Returns this App's shared typed event router.
    #[must_use]
    pub fn events(&self) -> AppEvents {
        self.events.clone()
    }

    /// Starts `AppKit` and blocks until the last MVP window closes.
    ///
    /// This path does not bind the optional [`Eventful`] capability. Components implementing it
    /// must be launched with [`run_eventful`](Self::run_eventful).
    ///
    /// # Errors
    ///
    /// Returns startup, rendering, or guarded render-loop failures.
    pub fn run<C: Render>(self, root: C) -> Result<(), AppError> {
        self.run_internal(WindowRoot::new(root))
    }

    /// Starts `AppKit` with the root Element's optional [`Eventful`] capability enabled.
    ///
    /// [`Eventful::bind_events`] is invoked once after the first frame is painted. The returned
    /// subscriptions remain owned by the root Element and are removed when it unmounts.
    ///
    /// # Errors
    ///
    /// Returns startup, rendering, or guarded render-loop failures.
    pub fn run_eventful<C: Render + Eventful>(self, root: C) -> Result<(), AppError> {
        self.run_internal(WindowRoot::new_eventful(root))
    }

    fn run_internal(self, root: WindowRoot) -> Result<(), AppError> {
        let mtm = MainThreadMarker::new().ok_or(AppError::NotMainThread)?;
        let session = Rc::new(AppSession::new(
            self.name,
            self.state,
            self.events,
            self.typography,
        )?);
        session.install_dispatcher();
        let _initial_window = session.open_window(self.window, root)?;
        ACTIVE_APP.with(|active| {
            active.replace(Some(session.clone()));
        });

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        let delegate = AppDelegate::new(mtm);
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        app.run();

        ACTIVE_APP.with(|active| {
            active.take();
        });
        session.shutdown();
        session.take_error().map_or(Ok(()), Err)
    }
}

const MAX_PENDING_WINDOW_COMMANDS: usize = 1_024;

enum WindowCommand {
    Open {
        id: WindowId,
        window: Window,
        root: WindowRoot,
        handle: WindowHandle,
    },
    Update {
        id: WindowId,
        update: WindowUpdate,
    },
    Action {
        id: WindowId,
        action: WindowAction,
    },
}

struct WindowEntry {
    window: Retained<NSWindow>,
    view: Retained<AnmixiuView>,
    _delegate: Retained<WindowDelegate>,
    driver: Rc<dyn NativeDriver>,
    handle: WindowHandle,
}

struct AppSession {
    name: SharedString,
    state: AppStateStore,
    events: AppEvents,
    typography: Typography,
    runtime: Rc<AppRuntime>,
    dispatcher: OnceCell<Weak<dyn WindowDispatcher>>,
    next_window_id: Cell<u64>,
    pending: RefCell<VecDeque<WindowCommand>>,
    closed_windows: RefCell<VecDeque<WindowId>>,
    draining: Cell<bool>,
    windows: RefCell<HashMap<WindowId, WindowEntry>>,
    handles: RefCell<HashMap<WindowId, WindowHandle>>,
    active_window: Cell<Option<WindowId>>,
    error: RefCell<Option<AppError>>,
}

impl AppSession {
    fn new(
        name: SharedString,
        state: AppStateStore,
        events: AppEvents,
        typography: Typography,
    ) -> Result<Self, AppError> {
        Ok(Self {
            name,
            state,
            events,
            typography,
            runtime: Rc::new(AppRuntime::new(wake_appkit)?),
            dispatcher: OnceCell::new(),
            next_window_id: Cell::new(1),
            pending: RefCell::new(VecDeque::new()),
            closed_windows: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
            windows: RefCell::new(HashMap::new()),
            handles: RefCell::new(HashMap::new()),
            active_window: Cell::new(None),
            error: RefCell::new(None),
        })
    }

    fn install_dispatcher(self: &Rc<Self>) {
        let dispatcher: Rc<dyn WindowDispatcher> = self.clone();
        let installed = self.dispatcher.set(Rc::downgrade(&dispatcher));
        debug_assert!(installed.is_ok(), "window dispatcher is installed once");
    }

    fn app_handle(&self) -> AppHandle {
        self.dispatcher
            .get()
            .cloned()
            .map_or_else(AppHandle::disconnected, AppHandle::new)
    }

    fn enqueue(&self, command: WindowCommand) -> Result<(), WindowError> {
        let mut pending = self.pending.borrow_mut();
        if pending.len() >= MAX_PENDING_WINDOW_COMMANDS {
            return Err(WindowError::CommandQueueFull);
        }
        pending.push_back(command);
        drop(pending);
        wake_appkit();
        Ok(())
    }

    fn wake(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            self.fail(AppError::NotMainThread);
            return;
        };
        self.drain_closed_windows();
        if let Err(error) = self.runtime.ui().run_ready() {
            self.fail(AppError::UiThread(error.to_string()));
            return;
        }
        self.drain_commands(mtm);
        self.drain_closed_windows();
        let drivers: Vec<_> = self
            .windows
            .borrow()
            .iter()
            .map(|(id, entry)| (*id, entry.driver.clone()))
            .collect();
        // `AppRuntime` is shared by every window, so it is drained once above. Driver wake only
        // maps each window's dirty owner set to its own display link.
        for (id, driver) in drivers {
            if driver.is_dirty() {
                self.request_display(id);
            }
        }
        self.drain_commands(mtm);
    }

    fn drain_commands(&self, mtm: MainThreadMarker) {
        if self.draining.replace(true) {
            return;
        }
        let result = drain_reentrant_queue(&self.pending, |command| match command {
            WindowCommand::Open {
                id,
                window,
                root,
                handle,
            } => {
                let result = self.open_native_window(mtm, id, window, root, &handle);
                if result.is_err() {
                    let mut info = handle.info();
                    info.status = WindowStatus::Closed;
                    handle.replace_info(info);
                    self.handles.borrow_mut().remove(&id);
                }
                result
            }
            WindowCommand::Update { id, update } => self.apply_update(id, &update),
            WindowCommand::Action { id, action } => self.apply_action(id, action),
        });
        self.draining.set(false);
        if let Err(error) = result {
            self.fail(error);
        }
    }

    fn drain_commands_if_idle(&self, mtm: MainThreadMarker) {
        if !self.draining.get() {
            self.drain_commands(mtm);
        }
    }

    fn open_native_window(
        &self,
        mtm: MainThreadMarker,
        id: WindowId,
        window: Window,
        root: WindowRoot,
        handle: &WindowHandle,
    ) -> Result<(), AppError> {
        let window = window.into_parts();
        let title = window.title.unwrap_or_else(|| self.name.clone());
        let typography = window.typography.with_fallback(&self.typography);
        let driver: Rc<dyn NativeDriver> = Rc::new(ComponentDriver::new(
            root,
            self.state.clone(),
            window.state,
            title.as_str(),
            font_spec(&typography),
            self.events.clone(),
            self.app_handle(),
            handle.clone(),
            self.runtime.clone(),
        )?);
        let content_size = window.content_size;
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(
                f64::from(content_size.width().value()),
                f64::from(content_size.height().value()),
            ),
        );
        // SAFETY: The session retains the window and disables automatic release-on-close below.
        let native_window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: Required for an NSWindow retained by the Rust WindowEntry.
        unsafe { native_window.setReleasedWhenClosed(false) };
        native_window.setTitle(&NSString::from_str(title.as_str()));
        native_window.setAcceptsMouseMovedEvents(true);
        native_window.setCollectionBehavior(
            NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenPrimary,
        );
        let delegate = WindowDelegate::new(mtm, id);
        native_window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let view = AnmixiuView::new(mtm, frame, id);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setWantsLayer(true);
        view.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
        let layer = MetalLayer::new();
        let raw_layer = std::ptr::from_ref::<metal::MetalLayerRef>(&layer)
            .cast_mut()
            .cast::<objc2::runtime::AnyObject>();
        // SAFETY: MetalLayerRef addresses its CAMetalLayer object, retained by NSView.
        unsafe {
            let _: () = msg_send![&view, setLayer: raw_layer];
        }
        native_window.setContentView(Some(&view));
        let _uses_display_link = view.install_display_link();
        let viewport = viewport_for_view(&view);
        driver.attach(viewport, layer);

        self.windows.borrow_mut().insert(
            id,
            WindowEntry {
                window: native_window.clone(),
                view,
                _delegate: delegate,
                driver: driver.clone(),
                handle: handle.clone(),
            },
        );
        native_window.center();
        native_window.makeKeyAndOrderFront(None);
        self.active_window.set(Some(id));
        handle.replace_info(WindowInfo {
            id,
            title,
            content_size: WindowSize::new(viewport.logical_size().0, viewport.logical_size().1),
            scale_factor: viewport.scale(),
            focused: true,
            visibility: WindowVisibility::Visible,
            mode: WindowMode::Windowed,
            status: WindowStatus::Open,
        });
        driver.draw();
        self.request_display(id);
        Ok(())
    }

    fn apply_update(&self, id: WindowId, update: &WindowUpdate) -> Result<(), AppError> {
        let entry = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| (entry.window.clone(), entry.handle.clone()))
            .ok_or(WindowError::Closed(id))?;
        let (window, handle) = entry;
        let next_title = match update.title_update() {
            PropertyUpdate::Keep => None,
            PropertyUpdate::Set(title) => {
                window.setTitle(&NSString::from_str(title.as_str()));
                Some(title.clone())
            }
            PropertyUpdate::Reset => {
                window.setTitle(&NSString::from_str(self.name.as_str()));
                Some(self.name.clone())
            }
        };
        let next_size = match update.content_size_update() {
            PropertyUpdate::Keep => None,
            PropertyUpdate::Set(size) => {
                window.setContentSize(NSSize::new(
                    f64::from(size.width().value()),
                    f64::from(size.height().value()),
                ));
                Some(*size)
            }
            PropertyUpdate::Reset => {
                let size = WindowSize::default();
                window.setContentSize(NSSize::new(
                    f64::from(size.width().value()),
                    f64::from(size.height().value()),
                ));
                Some(size)
            }
        };
        let mut info = handle.info();
        if let Some(title) = next_title {
            info.title = title;
        }
        if let Some(size) = next_size {
            info.content_size = size;
        }
        handle.replace_info(info);
        self.sync_window_info(id);
        Ok(())
    }

    fn apply_action(&self, id: WindowId, action: WindowAction) -> Result<(), AppError> {
        let window = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| entry.window.clone())
            .ok_or(WindowError::Closed(id))?;
        match action {
            WindowAction::Focus => window.makeKeyAndOrderFront(None),
            WindowAction::Minimize => window.miniaturize(None),
            WindowAction::Maximize => {
                if !window.isZoomed() {
                    window.zoom(None);
                }
            }
            WindowAction::Restore => {
                if window.isMiniaturized() {
                    window.deminiaturize(None);
                }
                if window.isZoomed() {
                    window.zoom(None);
                }
            }
            WindowAction::Close => window.performClose(None),
        }
        self.sync_window_info(id);
        Ok(())
    }

    fn request_display(&self, id: WindowId) {
        let target = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| (entry.view.clone(), entry.driver.clone()));
        let Some((view, driver)) = target else {
            return;
        };
        view.queue_display();
        if !view.resume_display_link() {
            with_driver_resize(&view);
            driver.draw();
        }
    }

    fn sync_window_info(&self, id: WindowId) {
        let snapshot = self.windows.borrow().get(&id).map(|entry| {
            (
                entry.window.clone(),
                entry.view.clone(),
                entry.handle.clone(),
            )
        });
        let Some((window, view, handle)) = snapshot else {
            return;
        };
        let viewport = viewport_for_view(&view);
        let mut info = handle.info();
        info.content_size = WindowSize::new(
            viewport.logical_size().0.max(f32::EPSILON),
            viewport.logical_size().1.max(f32::EPSILON),
        );
        info.scale_factor = viewport.scale();
        info.focused = window.isKeyWindow();
        info.visibility = if window.isMiniaturized() {
            WindowVisibility::Minimized
        } else if window.isVisible() {
            WindowVisibility::Visible
        } else {
            WindowVisibility::Hidden
        };
        info.mode = if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
            WindowMode::Fullscreen
        } else if window.isZoomed() {
            WindowMode::Maximized
        } else {
            WindowMode::Windowed
        };
        info.status = WindowStatus::Open;
        handle.replace_info(info);
    }

    fn window_focused(&self, id: WindowId, focused: bool) {
        if focused {
            self.active_window.set(Some(id));
        } else if self.active_window.get() == Some(id) {
            self.active_window.set(None);
        }
        self.sync_window_info(id);
        self.schedule_dirty_windows();
    }

    fn window_changed(&self, id: WindowId) {
        let target = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| (entry.view.clone(), entry.driver.clone()));
        if let Some((view, driver)) = target {
            let viewport = viewport_for_view(&view);
            driver.resize(viewport);
            self.sync_window_info(id);
            self.schedule_dirty_windows();
        }
    }

    fn window_closed(&self, id: WindowId) {
        let entry = self.windows.borrow_mut().remove(&id);
        let Some(entry) = entry else {
            return;
        };
        entry.driver.shutdown();
        if let Some(error) = entry.driver.take_error() {
            self.record_error(error);
        }
        let mut info = entry.handle.info();
        info.status = WindowStatus::Closed;
        info.visibility = WindowVisibility::Hidden;
        info.focused = false;
        entry.handle.replace_info(info);
        self.handles.borrow_mut().remove(&id);
        if self.active_window.get() == Some(id) {
            self.active_window.set(None);
        }
        if self.windows.borrow().is_empty() && self.pending.borrow().is_empty() {
            terminate_application();
        }
    }

    fn defer_window_closed(&self, id: WindowId) {
        self.closed_windows.borrow_mut().push_back(id);
        wake_appkit();
    }

    fn drain_closed_windows(&self) {
        loop {
            let id = self.closed_windows.borrow_mut().pop_front();
            let Some(id) = id else {
                break;
            };
            self.window_closed(id);
        }
    }

    fn schedule_dirty_windows(&self) {
        let dirty: Vec<_> = self
            .windows
            .borrow()
            .iter()
            .filter_map(|(id, entry)| entry.driver.is_dirty().then_some(*id))
            .collect();
        for id in dirty {
            self.request_display(id);
        }
    }

    fn fail(&self, error: AppError) {
        self.record_error(error);
        terminate_application();
    }

    fn record_error(&self, error: AppError) {
        let mut pending = self.error.borrow_mut();
        if pending.is_none() {
            eprintln!("Anmixiu stopped after an unrecoverable error: {error}");
            *pending = Some(error);
        }
    }

    fn take_error(&self) -> Option<AppError> {
        self.error.borrow_mut().take()
    }

    fn shutdown(&self) {
        let entries: Vec<_> = self
            .windows
            .borrow_mut()
            .drain()
            .map(|(_, entry)| entry)
            .collect();
        for entry in entries {
            entry.driver.shutdown();
            if let Some(error) = entry.driver.take_error() {
                self.record_error(error);
            }
            let mut info = entry.handle.info();
            info.status = WindowStatus::Closed;
            info.visibility = WindowVisibility::Hidden;
            info.focused = false;
            entry.handle.replace_info(info);
        }
        self.handles.borrow_mut().clear();
        self.pending.borrow_mut().clear();
        self.closed_windows.borrow_mut().clear();
    }
}

fn drain_reentrant_queue<T, E>(
    queue: &RefCell<VecDeque<T>>,
    mut operation: impl FnMut(T) -> Result<(), E>,
) -> Result<(), E> {
    loop {
        // Bind the result before invoking user/lifecycle work so the mutable queue borrow is gone.
        let item = queue.borrow_mut().pop_front();
        let Some(item) = item else {
            return Ok(());
        };
        operation(item)?;
    }
}

impl WindowDispatcher for AppSession {
    fn open_window(&self, window: Window, root: WindowRoot) -> Result<WindowHandle, WindowError> {
        if self.pending.borrow().len() >= MAX_PENDING_WINDOW_COMMANDS {
            return Err(WindowError::CommandQueueFull);
        }
        let raw_id = self.next_window_id.get();
        let next = raw_id.checked_add(1).ok_or(WindowError::IdExhausted)?;
        let id = WindowId::new(raw_id);
        let title = window
            .requested_title()
            .cloned()
            .unwrap_or_else(|| self.name.clone());
        let content_size = window.content_size();
        let dispatcher = self
            .dispatcher
            .get()
            .cloned()
            .ok_or(WindowError::AppStopped)?;
        let handle = WindowHandle::new(
            id,
            dispatcher,
            WindowInfo {
                id,
                title,
                content_size,
                scale_factor: 1.0,
                focused: false,
                visibility: WindowVisibility::Hidden,
                mode: WindowMode::Windowed,
                status: WindowStatus::Opening,
            },
        );
        self.enqueue(WindowCommand::Open {
            id,
            window,
            root,
            handle: handle.clone(),
        })?;
        self.next_window_id.set(next);
        self.handles.borrow_mut().insert(id, handle.clone());
        Ok(handle)
    }

    fn update_window(&self, id: WindowId, update: WindowUpdate) -> Result<(), WindowError> {
        if !self.handles.borrow().contains_key(&id) {
            return Err(WindowError::Closed(id));
        }
        self.enqueue(WindowCommand::Update { id, update })
    }

    fn window_action(&self, id: WindowId, action: WindowAction) -> Result<(), WindowError> {
        if !self.handles.borrow().contains_key(&id) {
            return Err(WindowError::Closed(id));
        }
        self.enqueue(WindowCommand::Action { id, action })?;
        if action == WindowAction::Close
            && let Some(handle) = self.handles.borrow().get(&id)
        {
            let mut info = handle.info();
            info.status = WindowStatus::Closing;
            handle.replace_info(info);
        }
        Ok(())
    }

    fn windows(&self) -> Vec<WindowHandle> {
        let mut handles: Vec<_> = self.handles.borrow().values().cloned().collect();
        handles.sort_by_key(WindowHandle::id);
        handles
    }

    fn active_window(&self) -> Option<WindowHandle> {
        let id = self.active_window.get()?;
        self.handles.borrow().get(&id).cloned()
    }
}

fn font_spec(typography: &Typography) -> FontSpec {
    match (typography.font_family(), typography.font_size()) {
        (Some(family), Some(size)) => FontSpec::new(family.as_str(), size.value()),
        (Some(family), None) => FontSpec::named_default(family.as_str()),
        (None, Some(size)) => FontSpec::system_ui(size.value()),
        (None, None) => FontSpec::system_ui_default(),
    }
}

trait NativeDriver {
    fn attach(&self, viewport: Viewport, layer: MetalLayer);
    fn draw(&self);
    fn resize(&self, viewport: Viewport);
    fn pointer_moved(&self, point: Point);
    fn cursor_style_at(&self, point: Point) -> CursorStyle;
    fn pointer_exited(&self);
    fn pointer_down(&self, point: Point);
    fn pointer_up(&self, point: Point);
    fn scroll(&self, point: Point, delta_x: f32, delta_y: f32);
    fn shutdown(&self);
    fn is_dirty(&self) -> bool;
    fn take_error(&self) -> Option<AppError>;
}

struct DriverState {
    window_id: WindowId,
    runtime: Rc<AppRuntime>,
    owners: OwnerRegistry,
    owner: OwnerId,
    host: Box<dyn ErasedComponentHost>,
    #[cfg(debug_assertions)]
    root_type: &'static str,
    frame_builder: FrameBuilder,
    renderer: MetalRenderer,
    layer: Option<MetalLayer>,
    viewport: Viewport,
    configured_viewport: Option<Viewport>,
    frame: Option<BuiltFrame>,
    pointer: PointerTracker,
    pressed_element: Option<GlobalElementId>,
    needs_frame: bool,
    drawable_retry_armed: bool,
    invalidation_streak: usize,
    stalled: HashSet<OwnerId>,
    #[cfg(debug_assertions)]
    animation_frame_streak: u64,
    last_draw_at: Option<Instant>,
    error: Option<AppError>,
    #[cfg(feature = "devtools")]
    devtools: DevToolsAgent,
    #[cfg(feature = "devtools")]
    devtools_commands: tokio::sync::mpsc::Receiver<DevToolsCommand>,
}

impl DriverState {
    fn handle_frame_outcome(&mut self, outcome: FrameOutcome) -> bool {
        match outcome {
            FrameOutcome::Presented => {
                self.needs_frame = false;
                self.drawable_retry_armed = false;
                self.host.did_paint();
                false
            }
            FrameOutcome::DrawableUnavailable { .. } => {
                self.needs_frame = true;
                take_drawable_retry_slot(&mut self.drawable_retry_armed)
            }
            FrameOutcome::SurfaceOutOfDate { .. } => {
                self.needs_frame = true;
                self.drawable_retry_armed = false;
                true
            }
        }
    }
}

struct ComponentDriver {
    state: RefCell<DriverState>,
}

impl ComponentDriver {
    #[allow(clippy::too_many_arguments)]
    fn new(
        root: WindowRoot,
        app_state: AppStateStore,
        window_state: WindowStateStore,
        app_name: &str,
        font: FontSpec,
        app_events: AppEvents,
        app_handle: AppHandle,
        window_handle: WindowHandle,
        runtime: Rc<AppRuntime>,
    ) -> Result<Self, AppError> {
        #[cfg(debug_assertions)]
        let root_type = root.root_type();
        let window_id = window_handle.id();
        #[cfg(feature = "devtools")]
        let (devtools, devtools_commands) =
            DevToolsAgent::connect(runtime.tokio_handle(), app_name, wake_appkit);
        #[cfg(not(feature = "devtools"))]
        let _ = app_name;
        let owners = OwnerRegistry::new();
        let spawner = runtime.ui().spawner(owners.clone());
        let mounted = root.mount(WindowMountContext {
            app_state,
            window_state,
            app_events,
            app_handle,
            window_handle,
            owners: owners.clone(),
            spawner,
        });
        let renderer = MetalRenderer::new()?.ok_or(AppError::MetalUnavailable)?;
        Ok(Self {
            state: RefCell::new(DriverState {
                window_id,
                runtime,
                owners,
                owner: mounted.owner,
                host: mounted.host,
                #[cfg(debug_assertions)]
                root_type,
                frame_builder: FrameBuilder::new_with_font(font)?,
                renderer,
                layer: None,
                viewport: Viewport::new(1.0, 1.0, 1.0),
                configured_viewport: None,
                frame: None,
                pointer: PointerTracker::default(),
                pressed_element: None,
                needs_frame: true,
                drawable_retry_armed: false,
                invalidation_streak: 0,
                stalled: HashSet::new(),
                #[cfg(debug_assertions)]
                animation_frame_streak: 0,
                last_draw_at: None,
                error: None,
                #[cfg(feature = "devtools")]
                devtools,
                #[cfg(feature = "devtools")]
                devtools_commands,
            }),
        })
    }

    fn fail(&self, error: AppError) {
        let mut state = self.state.borrow_mut();
        if state.error.is_none() {
            eprintln!("Anmixiu stopped after an unrecoverable error: {error}");
            state.error = Some(error);
        }
        drop(state);
        terminate_application();
    }

    fn take_error(&self) -> Option<AppError> {
        self.state.borrow_mut().error.take()
    }

    fn schedule_if_dirty(&self) {
        let state = self.state.borrow();
        if state.owners.dirty_len() != 0 {
            request_display(state.window_id);
        }
    }

    fn request_display(&self) {
        request_display(self.state.borrow().window_id);
    }

    fn surface_size(viewport: Viewport) -> SurfaceSize {
        let (width, height) = viewport.physical_size();
        SurfaceSize::new(width.max(1), height.max(1)).expect("clamped dimensions are non-zero")
    }

    /// Warns once when an animation has been running long enough to look like a
    /// `request_animation_frame` that forgot to stop. Debug-only: it never affects rendering, since
    /// display-link pacing already bounds the cost. The message names the root component type and
    /// the owner ids still animating so the culprit is identifiable.
    #[cfg(debug_assertions)]
    fn warn_on_runaway_animation(
        state: &mut DriverState,
        animating: &[(OwnerId, &'static std::panic::Location<'static>)],
    ) {
        if animating.is_empty() {
            state.animation_frame_streak = 0;
            return;
        }
        state.animation_frame_streak = state.animation_frame_streak.saturating_add(1);
        if state.animation_frame_streak == RUNAWAY_ANIMATION_WARN_FRAMES {
            // Report the source location of each still-animating request so the offending
            // `request_animation_frame` call in user code is directly identifiable.
            let call_sites: Vec<String> = animating
                .iter()
                .map(|(owner, site)| format!("{owner:?} at {site}"))
                .collect();
            tracing::warn!(
                component = state.root_type,
                call_sites = ?call_sites,
                frames = state.animation_frame_streak,
                "component has requested an animation frame every frame for a long time; \
                 ensure it stops calling request_animation_frame when the animation completes"
            );
        }
    }
}

impl NativeDriver for ComponentDriver {
    fn attach(&self, viewport: Viewport, layer: MetalLayer) {
        let mut state = self.state.borrow_mut();
        state.viewport = viewport;
        state.renderer.configure_layer(
            &layer,
            Self::surface_size(state.viewport),
            state.viewport.scale(),
        );
        state.layer = Some(layer);
        state.configured_viewport = Some(viewport);
        state.needs_frame = true;
    }

    #[allow(clippy::too_many_lines)]
    fn draw(&self) {
        let mut retry_surface_on_next_tick = false;
        let mut scrolling = false;
        let result = (|| -> Result<(), AppError> {
            let mut state = self.state.borrow_mut();
            // Termination is queued asynchronously by `fail()`, so a display-link tick or main-queue
            // wake can re-enter `draw()` after a render error has already been recorded. The layer
            // may have been taken and not restored on that error path; bail out instead of hitting
            // the "view attached before drawing" expect during teardown.
            if state.error.is_some() {
                return Ok(());
            }
            let now = Instant::now();
            let delta_seconds = state
                .last_draw_at
                .replace(now)
                .map_or(1.0 / 60.0, |previous| {
                    previous.elapsed().as_secs_f32().clamp(1.0 / 240.0, 0.1)
                });
            if let Some(frame) = state.frame.as_ref()
                && frame.advance_scroll(delta_seconds)
            {
                state.frame_builder.note_scrolled();
                state.needs_frame = true;
                scrolling = true;
            }
            #[cfg(feature = "devtools")]
            let mut request_tree = false;
            #[cfg(feature = "devtools")]
            while let Ok(command) = state.devtools_commands.try_recv() {
                match command {
                    DevToolsCommand::RequestTree => request_tree = true,
                    DevToolsCommand::Preview(element) => {
                        state.frame_builder.set_previewed(Some(element));
                        state.needs_frame = true;
                    }
                    DevToolsCommand::PreviewNode(node) => {
                        state.frame_builder.set_previewed_node(Some(node));
                        state.needs_frame = true;
                    }
                    DevToolsCommand::ClearPreview => {
                        state.frame_builder.clear_preview();
                        state.needs_frame = true;
                    }
                    DevToolsCommand::Inspect(element) => {
                        state.frame_builder.set_inspected(Some(element));
                        state.needs_frame = true;
                        request_tree = true;
                    }
                    DevToolsCommand::InspectNode(node) => {
                        state.frame_builder.set_inspected_node(Some(node));
                        state.needs_frame = true;
                        request_tree = true;
                    }
                    DevToolsCommand::ClearInspection => {
                        state.frame_builder.set_inspected(None);
                        state.needs_frame = true;
                        request_tree = true;
                    }
                }
            }
            let dirty = state.owners.take_dirty();
            // `take_dirty` returns the owners that were dirty *before* this render ran (external
            // drivers: clicks, timers, resize-driven state). A stalled owner reappearing here was
            // therefore re-dirtied by an outside event, not by its own render loop, so it has
            // recovered — un-stall it and let it draw again.
            if !state.stalled.is_empty() {
                for owner in &dirty {
                    if state.stalled.remove(owner) {
                        state.invalidation_streak = 0;
                    }
                }
            }
            let owner_stalled = state.stalled.contains(&state.owner);
            let rerender =
                !owner_stalled && (state.frame.is_none() || dirty.contains(&state.owner));
            if rerender {
                state
                    .host
                    .render()
                    .map_err(|error| AppError::Component(error.to_string()))?;
                state.needs_frame = true;
            }
            if !state.needs_frame {
                #[cfg(feature = "devtools")]
                if request_tree
                    && let Some(element) = state.host.element_snapshot()
                    && let Some(frame) = state.frame.as_ref()
                {
                    state.devtools.publish_tree(element.as_ref(), frame);
                }
                return Ok(());
            }
            let element = state
                .host
                .element_snapshot()
                .expect("a requested frame has a rendered root");
            let logical = state.viewport.logical_size();
            let scale = state.viewport.scale();
            let mut frame = state.frame_builder.build(
                element.as_ref(),
                Size::new(logical.0, logical.1),
                scale,
            )?;
            let hover_point = state.pointer.is_inside().then(|| {
                let (x, y) = state.pointer.position();
                Point::new(x, y)
            });
            if state.frame_builder.update_hover(&frame, hover_point) {
                frame = state.frame_builder.build(
                    element.as_ref(),
                    Size::new(logical.0, logical.1),
                    scale,
                )?;
            }
            #[cfg(feature = "devtools")]
            state.devtools.publish_tree(element.as_ref(), &frame);
            state.frame = Some(frame);
            let layer = state.layer.take().expect("view attached before drawing");
            if state.configured_viewport != Some(state.viewport) {
                state.renderer.configure_layer(
                    &layer,
                    Self::surface_size(state.viewport),
                    state.viewport.scale(),
                );
                state.configured_viewport = Some(state.viewport);
            }
            let frame = state.frame.take().expect("frame was just built");
            let outcome = state.renderer.render_layer(&layer, &frame.scene, scale)?;
            state.layer = Some(layer);
            state.frame = Some(frame);
            retry_surface_on_next_tick = state.handle_frame_outcome(outcome);

            // Separate declared animation from anonymous self-invalidation. `take_animating`
            // consumes the owners that called `request_animation_frame` during this render; every
            // owner still dirty after render that is NOT in that set re-dirtied itself without
            // declaring an animation, which is the runaway-loop signature we guard against.
            // Animation frames are exempt: they are paced by the display link (vsync), so a
            // continuous animation drives one frame per refresh rather than spinning.
            let animating_sites = state.owners.take_animating_with_sites();
            let animating: Vec<OwnerId> = animating_sites.iter().map(|(owner, _)| *owner).collect();
            let self_invalidators =
                anonymous_self_invalidators(&state.owners.dirty_snapshot(), &animating);

            #[cfg(debug_assertions)]
            Self::warn_on_runaway_animation(&mut state, &animating_sites);

            match evaluate_runaway_guard(
                &mut state.invalidation_streak,
                !self_invalidators.is_empty(),
            ) {
                GuardDecision::Settled | GuardDecision::Watching => {}
                GuardDecision::Tripped => {
                    // Debug builds fail fast so the offending component is caught during
                    // development. Release builds keep the app alive: freeze just these owners and
                    // drop their pending frames so they stop driving redraws, and log loudly. A
                    // later external event (click/timer) re-dirties the owner via `take_dirty` at
                    // the top of `draw`, which un-stalls it and lets it recover.
                    #[cfg(debug_assertions)]
                    return Err(AppError::RenderLoop(state.invalidation_streak));
                    #[cfg(not(debug_assertions))]
                    {
                        let streak = state.invalidation_streak;
                        for owner in &self_invalidators {
                            let _ = state.owners.clear_dirty(*owner);
                            state.stalled.insert(*owner);
                        }
                        tracing::error!(
                            streak,
                            frozen_owners = self_invalidators.len(),
                            "component invalidated itself every frame without requesting an \
                             animation frame; freezing it to stop a render loop"
                        );
                        state.invalidation_streak = 0;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
            return;
        }
        if retry_surface_on_next_tick {
            self.request_display();
        }
        if scrolling {
            self.request_display();
        }
        self.schedule_if_dirty();
    }

    fn resize(&self, viewport: Viewport) {
        let mut state = self.state.borrow_mut();
        if state.viewport == viewport {
            return;
        }
        state.viewport = viewport;
        state.needs_frame = true;
        state.drawable_retry_armed = false;
        // A layer is attached only after `attach()`. During window setup `setFrameSize:` can fire
        // before then, so fall back to a deferred display request until the layer exists.
        let attached = state.layer.is_some();
        drop(state);
        if attached {
            // Redraw synchronously inside the current AppKit turn. During a live resize this is
            // the nested `setFrameSize:` transaction that just applied the new layer bounds, so
            // relayout and drawable present land in that same transaction. Deferring to a later
            // display-link tick leaves content one turn behind the bounds, which is the jitter
            // that gets worse the faster the window is dragged.
            self.draw();
        } else {
            self.request_display();
        }
    }

    fn pointer_moved(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let changed = {
            let DriverState {
                frame_builder,
                frame,
                ..
            } = &mut *state;
            frame
                .as_ref()
                .is_some_and(|frame| frame_builder.update_hover(frame, Some(point)))
        };
        if changed {
            state.needs_frame = true;
        }
        drop(state);
        if changed {
            self.request_display();
        }
    }

    fn cursor_style_at(&self, point: Point) -> CursorStyle {
        let state = self.state.borrow();
        let Some(frame) = state.frame.as_ref() else {
            return CursorStyle::Default;
        };
        frame
            .scene
            .hit_test(point)
            .map_or(CursorStyle::Default, |hit| frame.cursor_style(hit))
    }

    fn pointer_exited(&self) {
        let mut state = self.state.borrow_mut();
        state.pointer.exit();
        let changed = {
            let DriverState {
                frame_builder,
                frame,
                ..
            } = &mut *state;
            frame
                .as_ref()
                .is_some_and(|frame| frame_builder.update_hover(frame, None))
        };
        if changed {
            state.needs_frame = true;
        }
        drop(state);
        if changed {
            self.request_display();
        }
    }

    fn pointer_down(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let hit = state
            .frame
            .as_ref()
            .and_then(|frame| frame.click_target_at(point));
        let focused = hit.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.global_id(hit).cloned())
        });
        state.pressed_element.clone_from(&focused);
        state.pointer.press(hit.map(|hit| hit.0));
        let focus_changed = {
            let DriverState {
                frame_builder,
                frame,
                ..
            } = &mut *state;
            frame
                .as_ref()
                .is_some_and(|frame| frame_builder.focus_at(frame, point))
        };
        if focus_changed {
            state.needs_frame = true;
        }
        drop(state);
        if focus_changed {
            self.request_display();
        }
    }

    fn pointer_up(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let target = state
            .frame
            .as_ref()
            .and_then(|frame| frame.click_target_at(point));
        let current_element = target.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.global_id(hit).cloned())
        });
        let _transient_click = state.pointer.release(target.map(|hit| hit.0));
        let pressed_element = state.pressed_element.take();
        let clicked = (pressed_element.is_some() && pressed_element == current_element)
            .then_some(target)
            .flatten();
        let handler = clicked.and_then(|clicked| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.handler(clicked).cloned())
        });
        if let Some(handler) = handler
            && let Some(future) = handler.invoke()
        {
            let owner = state.owner;
            if let Err(error) = state.runtime.ui().spawn(&state.owners, owner, future) {
                panic!("async click handler could not be scheduled: {error}");
            }
        }
        let dirty = state.owners.dirty_len() != 0;
        drop(state);
        if dirty {
            self.request_display();
        }
    }

    fn scroll(&self, point: Point, delta_x: f32, delta_y: f32) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        // Route the wheel delta to the scroll container under the cursor. Scroll offsets live in
        // app-owned handles read at scene-build time, so a change is not seen by the reactive
        // owner; bump the paint revision and repaint directly instead.
        let consumed = state
            .frame
            .as_ref()
            .is_some_and(|frame| frame.scroll_at_axes(point, delta_x, delta_y));
        if consumed {
            state.frame_builder.note_scrolled();
            state.needs_frame = true;
            // The display link pauses while idle, leaving `last_draw_at` far in the past; without
            // this the first animation frame would see a large (clamped-to-0.1s) dt and lurch ~86%
            // toward the target. Reset so the animation starts from a clean 1/60 step.
            state.last_draw_at = None;
        }
        drop(state);
        if consumed {
            self.request_display();
        }
    }

    fn shutdown(&self) {
        self.state.borrow_mut().host.unmount();
    }

    fn is_dirty(&self) -> bool {
        let state = self.state.borrow();
        state.needs_frame || state.owners.dirty_len() != 0
    }

    fn take_error(&self) -> Option<AppError> {
        ComponentDriver::take_error(self)
    }
}

/// Axis a trackpad scroll gesture has committed to, so minor cross-axis jitter is suppressed for
/// the rest of the gesture (matching native macOS scroll axis-locking).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ScrollAxisLock {
    #[default]
    Free,
    Horizontal,
    Vertical,
}

struct ViewIvars {
    window_id: WindowId,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
    display_link: RefCell<Option<Retained<CADisplayLink>>>,
    display_queued: Cell<bool>,
    scroll_axis_lock: Cell<ScrollAxisLock>,
}

impl ViewIvars {
    fn new(window_id: WindowId) -> Self {
        Self {
            window_id,
            tracking_area: RefCell::new(None),
            display_link: RefCell::new(None),
            display_queued: Cell::new(false),
            scroll_axis_lock: Cell::new(ScrollAxisLock::Free),
        }
    }
}

define_class!(
    // SAFETY: NSView supports subclassing, AnmixiuView has no Drop implementation, and every
    // exported method below matches its Objective-C selector signature.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    struct AnmixiuView;

    // SAFETY: NSObjectProtocol adds no invariants beyond the inherited NSObject behavior.
    unsafe impl NSObjectProtocol for AnmixiuView {}

    impl AnmixiuView {
        // SAFETY: Signature matches -[NSView isFlipped].
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        // SAFETY: Signature matches -[NSView acceptsFirstResponder].
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // SAFETY: Signature matches -[NSView drawRect:].
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            request_display(self.ivars().window_id);
        }

        // SAFETY: Signature matches the selector registered with CADisplayLink.
        #[unsafe(method(displayLinkTick:))]
        fn display_link_tick(&self, link: &CADisplayLink) {
            with_driver_resize(self);
            let window_id = self.ivars().window_id;
            if self.ivars().display_queued.replace(false) {
                with_driver(window_id, |driver| driver.draw());
            }
            if !self.ivars().display_queued.get() {
                link.setPaused(true);
            }
        }

        // SAFETY: Signature matches -[NSResponder mouseMoved:].
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            with_driver_point(self.ivars().window_id, point, PointOperation::Move);
            set_cursor_for_point(self.ivars().window_id, point);
        }

        // SAFETY: Signature matches -[NSResponder mouseExited:].
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            NSCursor::arrowCursor().set();
            with_driver(self.ivars().window_id, |driver| driver.pointer_exited());
        }

        // SAFETY: Signature matches -[NSResponder mouseDown:].
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            with_driver_point(self.ivars().window_id, point, PointOperation::Down);
        }

        // SAFETY: Signature matches -[NSResponder mouseUp:].
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            with_driver_point(self.ivars().window_id, point, PointOperation::Up);
        }

        // SAFETY: Signature matches -[NSResponder scrollWheel:].
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            // AppKit scrolling-up delta is positive; our offset grows as content moves up, so we
            // subtract. Precise (trackpad) deltas are already in points; line deltas are scaled.
            let precise = event.hasPreciseScrollingDeltas();
            let scale = if precise { 1.0 } else { 16.0 };
            let mut delta_x = event.scrollingDeltaX() * scale;
            let mut delta_y = event.scrollingDeltaY() * scale;

            // A new gesture resets the axis lock; the trackpad emits both axes at once, so a mostly
            // horizontal swipe still carries small vertical jitter (and vice versa).
            if event.phase().contains(NSEventPhase::Began) {
                self.ivars().scroll_axis_lock.set(ScrollAxisLock::Free);
            }
            if precise {
                let lock = self.ivars().scroll_axis_lock.get();
                let lock = if lock == ScrollAxisLock::Free {
                    // Commit to an axis once one clearly dominates, then keep it for the rest of the
                    // gesture (including the momentum phase, which carries no `Began`).
                    let committed = commit_scroll_axis(delta_x, delta_y);
                    if committed != ScrollAxisLock::Free {
                        self.ivars().scroll_axis_lock.set(committed);
                    }
                    committed
                } else {
                    lock
                };
                match lock {
                    ScrollAxisLock::Horizontal => delta_y = 0.0,
                    ScrollAxisLock::Vertical => delta_x = 0.0,
                    ScrollAxisLock::Free => {}
                }
            }

            // Match the browser convention for a mouse wheel: holding Shift turns a vertical
            // wheel tick into horizontal scrolling when the device did not provide an X delta.
            if delta_x.abs() <= f64::EPSILON
                && delta_y.abs() > f64::EPSILON
                && event
                    .modifierFlags()
                    .contains(NSEventModifierFlags::Shift)
            {
                delta_x = delta_y;
                delta_y = 0.0;
            }
            let point = Point::new(point.x as f32, point.y as f32);
            with_driver(self.ivars().window_id, |driver| {
                driver.scroll(point, -delta_x as f32, -delta_y as f32);
            });
        }

        // SAFETY: Signature matches -[NSView setFrameSize:] and calls the inherited implementation
        // before notifying the Rust driver.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: super dispatch targets NSView's implementation with the declared signature.
            unsafe { msg_send![super(self), setFrameSize: size] }
            with_driver_resize(self);
        }

        // SAFETY: Signature matches -[NSView viewDidChangeBackingProperties].
        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_changed(&self) {
            // SAFETY: super dispatch targets NSView's implementation with no arguments.
            unsafe { msg_send![super(self), viewDidChangeBackingProperties] }
            with_driver_resize(self);
        }

        // SAFETY: Signature matches -[NSView layer:shouldInheritContentsScale:fromWindow:].
        #[unsafe(method(layer:shouldInheritContentsScale:fromWindow:))]
        fn should_inherit_contents_scale(
            &self,
            _layer: &AnyObject,
            _new_scale: f64,
            _window: &NSWindow,
        ) -> bool {
            true
        }

        // SAFETY: Signature matches -[NSView updateTrackingAreas]. The inherited implementation
        // runs first, and AppKit retains the newly installed tracking area.
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // SAFETY: super dispatch targets NSView's implementation with no arguments.
            unsafe { msg_send![super(self), updateTrackingAreas] }
            if let Some(previous) = self.ivars().tracking_area.borrow_mut().take() {
                self.removeTrackingArea(&previous);
            }
            let options = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveInKeyWindow
                | NSTrackingAreaOptions::InVisibleRect;
            // SAFETY: `self` is the tracking owner, implements the matching mouse selectors, and
            // both the view and tracking area are main-thread-only AppKit objects.
            let tracking = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    options,
                    Some(self as &AnyObject),
                    None,
                )
            };
            self.addTrackingArea(&tracking);
            self.ivars().tracking_area.replace(Some(tracking));
        }
    }
);

impl AnmixiuView {
    fn new(mtm: MainThreadMarker, frame: NSRect, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars::new(window_id));
        // SAFETY: initWithFrame: is NSView's designated initializer and `this` is freshly allocated.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn install_display_link(&self) -> bool {
        if !self.respondsToSelector(sel!(displayLinkWithTarget:selector:)) {
            return false;
        }
        // SAFETY: `self` implements `displayLinkTick:` with the required single-link argument.
        let link = unsafe {
            self.displayLinkWithTarget_selector(self as &AnyObject, sel!(displayLinkTick:))
        };
        link.setPaused(true);
        // SAFETY: The main run loop and common mode are valid for this main-thread display link.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        self.ivars().display_link.replace(Some(link));
        true
    }

    fn resume_display_link(&self) -> bool {
        let link = self.ivars().display_link.borrow();
        let Some(link) = link.as_ref() else {
            return false;
        };
        link.setPaused(false);
        true
    }

    fn queue_display(&self) {
        self.ivars().display_queued.set(true);
    }
}

#[derive(Clone, Copy)]
enum PointOperation {
    Move,
    Down,
    Up,
}

fn with_driver(window_id: WindowId, operation: impl FnOnce(&Rc<dyn NativeDriver>)) {
    let driver = ACTIVE_APP.with(|active| {
        active.borrow().as_ref().and_then(|session| {
            session
                .windows
                .borrow()
                .get(&window_id)
                .map(|entry| entry.driver.clone())
        })
    });
    if let Some(driver) = driver {
        operation(&driver);
        with_app_session(|session| {
            if let Some(mtm) = MainThreadMarker::new() {
                session.drain_commands_if_idle(mtm);
            }
            session.schedule_dirty_windows();
        });
    }
}

fn with_driver_point(window_id: WindowId, point: NSPoint, operation: PointOperation) {
    with_driver(window_id, |driver| {
        let point = Point::new(point.x as f32, point.y as f32);
        match operation {
            PointOperation::Move => driver.pointer_moved(point),
            PointOperation::Down => driver.pointer_down(point),
            PointOperation::Up => driver.pointer_up(point),
        }
    });
}

fn set_cursor_for_point(window_id: WindowId, point: NSPoint) {
    let point = Point::new(point.x as f32, point.y as f32);
    let mut style = CursorStyle::Default;
    with_driver(window_id, |driver| {
        style = driver.cursor_style_at(point);
    });
    match style {
        CursorStyle::Default => NSCursor::arrowCursor().set(),
        CursorStyle::Pointer => NSCursor::pointingHandCursor().set(),
        CursorStyle::Text => NSCursor::IBeamCursor().set(),
    }
}

/// Decides which axis a trackpad gesture has committed to, or `Free` if neither yet dominates.
///
/// A gesture is locked to an axis once its delta on that axis exceeds a small floor and is clearly
/// larger than the other (2:1). Below that it stays free so a genuinely diagonal swipe is not
/// forced onto one axis prematurely.
fn commit_scroll_axis(delta_x: f64, delta_y: f64) -> ScrollAxisLock {
    const FLOOR: f64 = 0.1;
    const DOMINANCE: f64 = 2.0;
    let ax = delta_x.abs();
    let ay = delta_y.abs();
    if ax > FLOOR && ax >= ay * DOMINANCE {
        ScrollAxisLock::Horizontal
    } else if ay > FLOOR && ay >= ax * DOMINANCE {
        ScrollAxisLock::Vertical
    } else {
        ScrollAxisLock::Free
    }
}

fn take_drawable_retry_slot(armed: &mut bool) -> bool {
    if *armed {
        return false;
    }
    *armed = true;
    true
}

/// Owners still dirty after a render that did NOT request an animation frame this turn. These are
/// the anonymous self-invalidations the runaway guard acts on; declared animation owners are
/// excluded because their per-frame re-dirtying is intentional and vsync-paced.
fn anonymous_self_invalidators(dirty: &[OwnerId], animating: &[OwnerId]) -> Vec<OwnerId> {
    let animating: HashSet<OwnerId> = animating.iter().copied().collect();
    dirty
        .iter()
        .copied()
        .filter(|owner| !animating.contains(owner))
        .collect()
}

/// Outcome of one turn of the render-loop guard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuardDecision {
    /// No anonymous self-invalidation this turn; the streak was reset.
    Settled,
    /// Self-invalidation occurred but the streak is still under the limit.
    Watching,
    /// The streak reached the limit; the caller must fail (debug) or freeze (release).
    Tripped,
}

/// Advances `streak` for this turn and decides whether the runaway guard trips. Pure so the
/// branching (reset / advance / trip at the limit) is unit-testable without a GPU or `AppKit`.
fn evaluate_runaway_guard(streak: &mut usize, has_self_invalidation: bool) -> GuardDecision {
    if !has_self_invalidation {
        *streak = 0;
        return GuardDecision::Settled;
    }
    *streak = streak.saturating_add(1);
    if *streak >= MAX_RENDER_INVALIDATIONS {
        GuardDecision::Tripped
    } else {
        GuardDecision::Watching
    }
}

fn viewport_for_view(view: &NSView) -> Viewport {
    let logical = view.bounds().size;
    let backing = view.convertSizeToBacking(logical);
    let physical_width = backing.width.round().max(1.0) as u32;
    let physical_height = backing.height.round().max(1.0) as u32;
    let scale_x = if logical.width > 0.0 {
        backing.width / logical.width
    } else {
        1.0
    };
    let scale_y = if logical.height > 0.0 {
        backing.height / logical.height
    } else {
        1.0
    };
    let scale = scale_x.midpoint(scale_y) as f32;
    Viewport::with_backing_size(
        logical.width as f32,
        logical.height as f32,
        scale.max(f32::EPSILON),
        physical_width,
        physical_height,
    )
}

fn with_driver_resize(view: &AnmixiuView) {
    let viewport = viewport_for_view(view);
    with_driver(view.ivars().window_id, |driver| driver.resize(viewport));
}

#[derive(Debug, Default)]
struct DelegateIvars;

define_class!(
    // SAFETY: NSObject supports subclassing, AppDelegate has no Drop implementation, and the
    // delegate callbacks below match the AppKit protocol selector signatures.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct AppDelegate;

    // SAFETY: NSObjectProtocol imposes no additional safety requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: NSApplicationDelegate has no unsafe implementor invariants.
    unsafe impl NSApplicationDelegate for AppDelegate {
        // SAFETY: Signature matches applicationDidFinishLaunching:.
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let app = NSApplication::sharedApplication(self.mtm());
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            app.activate();
            ACTIVE_APP.with(|active| {
                if let Some(session) = active.borrow().as_ref() {
                    session.wake();
                }
            });
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars);
        // SAFETY: NSObject's init signature is correct and `this` is freshly allocated.
        unsafe { msg_send![super(this), init] }
    }
}

#[derive(Debug)]
struct WindowDelegateIvars {
    window_id: WindowId,
}

define_class!(
    // SAFETY: NSObject supports subclassing, WindowDelegate has no Drop implementation, and each
    // exported method below matches its NSWindowDelegate selector signature.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = WindowDelegateIvars]
    struct WindowDelegate;

    // SAFETY: NSObjectProtocol imposes no additional safety requirements.
    unsafe impl NSObjectProtocol for WindowDelegate {}

    // SAFETY: NSWindowDelegate has no unsafe implementor invariants.
    unsafe impl NSWindowDelegate for WindowDelegate {
        // SAFETY: Signature matches windowWillClose: and routes by this delegate's immutable id.
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            // Defer removal so the WindowEntry keeps this Objective-C delegate retained until the
            // current callback has returned to AppKit.
            with_app_session(|session| session.defer_window_closed(self.ivars().window_id));
        }

        // SAFETY: Signature matches windowDidBecomeKey:.
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            with_app_session(|session| session.window_focused(self.ivars().window_id, true));
        }

        // SAFETY: Signature matches windowDidResignKey:.
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            with_app_session(|session| session.window_focused(self.ivars().window_id, false));
        }

        // SAFETY: Signature matches windowDidResize:.
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            with_app_session(|session| session.window_changed(self.ivars().window_id));
        }

        // SAFETY: Signature matches windowDidMiniaturize:.
        #[unsafe(method(windowDidMiniaturize:))]
        fn window_did_miniaturize(&self, _notification: &NSNotification) {
            with_app_session(|session| {
                session.sync_window_info(self.ivars().window_id);
                session.schedule_dirty_windows();
            });
        }

        // SAFETY: Signature matches windowDidDeminiaturize:.
        #[unsafe(method(windowDidDeminiaturize:))]
        fn window_did_deminiaturize(&self, _notification: &NSNotification) {
            with_app_session(|session| session.window_changed(self.ivars().window_id));
        }

        // SAFETY: Signature matches windowDidEnterFullScreen:.
        #[unsafe(method(windowDidEnterFullScreen:))]
        fn window_did_enter_full_screen(&self, _notification: &NSNotification) {
            with_app_session(|session| session.window_changed(self.ivars().window_id));
        }

        // SAFETY: Signature matches windowDidExitFullScreen:.
        #[unsafe(method(windowDidExitFullScreen:))]
        fn window_did_exit_full_screen(&self, _notification: &NSNotification) {
            with_app_session(|session| session.window_changed(self.ivars().window_id));
        }

        // SAFETY: Signature matches windowDidChangeBackingProperties:.
        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, _notification: &NSNotification) {
            with_app_session(|session| session.window_changed(self.ivars().window_id));
        }
    }
);

impl WindowDelegate {
    fn new(mtm: MainThreadMarker, window_id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowDelegateIvars { window_id });
        // SAFETY: NSObject's init signature is correct and `this` is freshly allocated.
        unsafe { msg_send![super(this), init] }
    }
}

fn with_app_session(operation: impl FnOnce(&AppSession)) {
    ACTIVE_APP.with(|active| {
        if let Some(session) = active.borrow().as_ref() {
            operation(session);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        GuardDecision, MAX_RENDER_INVALIDATIONS, anonymous_self_invalidators,
        drain_reentrant_queue, evaluate_runaway_guard, take_drawable_retry_slot,
    };
    use anmixiu_reactive::OwnerRegistry;
    use std::{cell::RefCell, collections::VecDeque};

    #[test]
    fn window_command_queue_releases_its_borrow_before_reentrant_enqueue() {
        let queue = RefCell::new(VecDeque::from([1_u8]));
        let mut seen = Vec::new();
        drain_reentrant_queue(&queue, |command| {
            seen.push(command);
            if command == 1 {
                queue.borrow_mut().push_back(2);
            }
            Ok::<_, ()>(())
        })
        .expect("infallible command drain");

        assert_eq!(seen, vec![1, 2]);
    }

    #[test]
    fn drawable_miss_gets_one_follow_up_turn_without_busy_looping() {
        let mut armed = false;
        assert!(take_drawable_retry_slot(&mut armed));
        assert!(!take_drawable_retry_slot(&mut armed));
    }

    #[test]
    fn scroll_axis_commits_to_the_dominant_axis_and_stays_free_when_diagonal() {
        use super::{ScrollAxisLock, commit_scroll_axis};
        // Mostly-horizontal trackpad swipe with minor vertical jitter locks to horizontal.
        assert_eq!(commit_scroll_axis(30.0, 4.0), ScrollAxisLock::Horizontal);
        // Mostly-vertical locks to vertical.
        assert_eq!(commit_scroll_axis(-3.0, 40.0), ScrollAxisLock::Vertical);
        // A genuine diagonal (neither axis 2x the other) stays free.
        assert_eq!(commit_scroll_axis(20.0, 18.0), ScrollAxisLock::Free);
        // Tiny sub-floor deltas at gesture start do not commit yet.
        assert_eq!(commit_scroll_axis(0.05, 0.01), ScrollAxisLock::Free);
    }

    #[test]
    fn declared_animation_owners_are_excluded_from_self_invalidation() {
        let owners = OwnerRegistry::new();
        let animator = owners.create_owner();
        let looper = owners.create_owner();

        // Both are dirty after render, but only `animator` declared an animation frame.
        let dirty = vec![animator, looper];
        let animating = vec![animator];

        let flagged = anonymous_self_invalidators(&dirty, &animating);
        assert_eq!(flagged, vec![looper], "the animation frame must be exempt");
    }

    #[test]
    fn pure_animation_never_flags_self_invalidation() {
        let owners = OwnerRegistry::new();
        let animator = owners.create_owner();
        assert!(
            anonymous_self_invalidators(&[animator], &[animator]).is_empty(),
            "an owner that only animates is not a runaway loop"
        );
    }

    #[test]
    fn guard_resets_when_no_self_invalidation() {
        let mut streak = 5;
        assert_eq!(
            evaluate_runaway_guard(&mut streak, false),
            GuardDecision::Settled
        );
        assert_eq!(streak, 0);
    }

    #[test]
    fn guard_trips_only_after_the_consecutive_limit() {
        let mut streak = 0;
        // The first MAX-1 self-invalidating turns are watched, not tripped.
        for _ in 0..MAX_RENDER_INVALIDATIONS - 1 {
            assert_eq!(
                evaluate_runaway_guard(&mut streak, true),
                GuardDecision::Watching
            );
        }
        // The MAX-th consecutive turn trips the guard.
        assert_eq!(
            evaluate_runaway_guard(&mut streak, true),
            GuardDecision::Tripped
        );
        assert_eq!(streak, MAX_RENDER_INVALIDATIONS);
    }

    #[test]
    fn a_settled_turn_between_self_invalidations_prevents_tripping() {
        let mut streak = 0;
        for _ in 0..MAX_RENDER_INVALIDATIONS - 1 {
            assert_eq!(
                evaluate_runaway_guard(&mut streak, true),
                GuardDecision::Watching
            );
        }
        // One clean turn resets the streak, so the loop must build up all over again.
        assert_eq!(
            evaluate_runaway_guard(&mut streak, false),
            GuardDecision::Settled
        );
        assert_eq!(
            evaluate_runaway_guard(&mut streak, true),
            GuardDecision::Watching
        );
    }
}
