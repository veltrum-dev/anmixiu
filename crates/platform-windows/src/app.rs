#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    mem::size_of,
    rc::{Rc, Weak},
    sync::atomic::{AtomicU32, Ordering},
    time::Instant,
};

use anmixiu_core::{
    AppEvents, AppHandle, AppStateStore, CursorStyle, ErasedComponentHost, Eventful,
    GlobalElementId, Pixels, PropertyUpdate, Render, SharedString, Typography, Window,
    WindowAction, WindowDispatcher, WindowError, WindowHandle, WindowId, WindowInfo, WindowMode,
    WindowMountContext, WindowRoot, WindowSize, WindowStateStore, WindowStatus, WindowUpdate,
    WindowVisibility,
};
use anmixiu_reactive::{OwnerId, OwnerRegistry};
use anmixiu_render_d3d11::{D3d11Renderer, FrameOutcome, RenderError, SurfaceSize};
use anmixiu_runtime::{AppRuntime, RuntimeBuildError};
use anmixiu_scene::{Point, Size};
use anmixiu_text_directwrite::FontSpec;
use thiserror::Error;
use windows::{
    Win32::{
        Foundation::{
            E_ACCESSDENIED, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM,
            LRESULT, POINT, RECT, WPARAM,
        },
        Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT, ScreenToClient},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            HiDpi::{
                AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
            },
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW, HTCLIENT, IDC_ARROW,
                IDC_HAND, IDC_IBEAM, KillTimer, LoadCursorW, MSG, PostMessageW, PostQuitMessage,
                PostThreadMessageW, RegisterClassExW, SIZE_MAXIMIZED, SIZE_MINIMIZED, SW_MAXIMIZE,
                SW_MINIMIZE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
                SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow,
                TranslateMessage, WINDOW_EX_STYLE, WM_ACTIVATE, WM_APP, WM_CLOSE, WM_DESTROY,
                WM_DPICHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
                WM_MOUSEWHEEL, WM_PAINT, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_TIMER,
                WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::PCWSTR,
};

use crate::{BuiltFrame, FrameBuildError, FrameBuilder, PointerTracker, Viewport};

const MAX_RENDER_INVALIDATIONS: usize = 8;
const FRAME_TIMER_ID: usize = 0xA11;
const FRAME_INTERVAL_MILLIS: u32 = 8;
const WM_ANMIXIU_WAKE: u32 = WM_APP + 1;
const WM_ANMIXIU_REQUEST_FRAME: u32 = WM_APP + 2;
const WM_MOUSELEAVE_MESSAGE: u32 = 0x02A3;
const SHIFT_BUTTON_MASK: usize = 0x0004;

static UI_THREAD_ID: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static ACTIVE_APP: RefCell<Option<Rc<AppSession>>> = const { RefCell::new(None) };
}

fn wake_win32() {
    let thread_id = UI_THREAD_ID.load(Ordering::Acquire);
    if thread_id == 0 {
        return;
    }
    // SAFETY: The id belongs to the application UI thread for the entire App::run call. A thread
    // message is independent of individual HWND lifetimes, so closing one window cannot lose a
    // runtime wake intended for another.
    if let Err(error) =
        unsafe { PostThreadMessageW(thread_id, WM_ANMIXIU_WAKE, WPARAM(0), LPARAM(0)) }
    {
        tracing::debug!(%error, "Win32 UI thread stopped accepting runtime wake messages");
    }
}

struct UiThreadWakeRegistration;

impl UiThreadWakeRegistration {
    fn install() -> Self {
        // SAFETY: Reads the numeric identity of the current application UI thread.
        UI_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
        Self
    }
}

impl Drop for UiThreadWakeRegistration {
    fn drop(&mut self) {
        UI_THREAD_ID.store(0, Ordering::Release);
    }
}

fn request_display(window_id: WindowId) {
    with_app_session(|session| session.request_display(window_id));
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Runtime(#[from] RuntimeBuildError),
    #[error(transparent)]
    Frame(#[from] FrameBuildError),
    #[error(transparent)]
    D3d11(#[from] RenderError),
    #[error("Win32 operation failed: {0}")]
    Win32(String),
    #[error("component invalidated itself for more than {0} consecutive frame turns")]
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

    /// Creates one native Win32 window and blocks in its message loop until it closes.
    ///
    /// This path does not bind the optional [`Eventful`] capability. Components implementing it
    /// must be launched with [`run_eventful`](Self::run_eventful).
    ///
    /// # Errors
    ///
    /// Returns an error when runtime, window, text, or graphics initialization fails, when a
    /// native frame cannot be built or presented, or when the component exceeds the guarded
    /// self-invalidation limit.
    pub fn run<C: Render>(self, root: C) -> Result<(), AppError> {
        self.run_internal(WindowRoot::new(root))
    }

    /// Starts Win32 with the root Element's optional [`Eventful`] capability enabled.
    ///
    /// [`Eventful::bind_events`] is invoked once after the first frame is painted.
    ///
    /// # Errors
    ///
    /// Returns the same startup, rendering, and render-loop errors as [`run`](Self::run).
    pub fn run_eventful<C: Render + Eventful>(self, root: C) -> Result<(), AppError> {
        self.run_internal(WindowRoot::new_eventful(root))
    }

    fn run_internal(self, root: WindowRoot) -> Result<(), AppError> {
        // SAFETY: DPI awareness is selected before creating any HWND. ERROR_ACCESS_DENIED means a
        // manifest or the host process already selected awareness, so continuing is correct.
        let dpi_result =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if let Err(error) = dpi_result
            && error.code() != E_ACCESSDENIED
        {
            return Err(win32_error(error));
        }
        let _wake_registration = UiThreadWakeRegistration::install();
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
        session.drain_commands();
        session.take_error().map_or(Ok(()), Err)?;

        let message_result = run_message_loop();
        session.shutdown();
        ACTIVE_APP.with(|active| {
            active.take();
        });
        message_result?;
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
    hwnd: HWND,
    driver: Rc<dyn NativeDriver>,
    handle: WindowHandle,
    frame_request_queued: Cell<bool>,
    frame_timer_armed: Cell<bool>,
    mouse_leave_tracked: Cell<bool>,
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
    draining: Cell<bool>,
    windows: RefCell<HashMap<WindowId, WindowEntry>>,
    ids_by_hwnd: RefCell<HashMap<isize, WindowId>>,
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
            runtime: Rc::new(AppRuntime::new(wake_win32)?),
            dispatcher: OnceCell::new(),
            next_window_id: Cell::new(1),
            pending: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
            windows: RefCell::new(HashMap::new()),
            ids_by_hwnd: RefCell::new(HashMap::new()),
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
        wake_win32();
        Ok(())
    }

    fn wake(&self) {
        if let Err(error) = self.runtime.ui().run_ready() {
            self.fail(AppError::UiThread(error.to_string()));
            return;
        }
        self.drain_commands();
        let drivers: Vec<_> = self
            .windows
            .borrow()
            .iter()
            .map(|(id, entry)| (*id, entry.driver.clone()))
            .collect();
        // `AppRuntime` is shared by every window, so it is drained once above. Driver wake only
        // maps each window's dirty owner set to its own frame request.
        for (id, driver) in drivers {
            if driver.is_dirty() {
                self.request_display(id);
            }
        }
        self.drain_commands();
    }

    fn drain_commands(&self) {
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
                let result = self.open_native_window(id, window, root, &handle);
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

    fn drain_commands_if_idle(&self) {
        if !self.draining.get() {
            self.drain_commands();
        }
    }

    fn open_native_window(
        &self,
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
            font_spec(&typography),
            self.events.clone(),
            self.app_handle(),
            handle.clone(),
            self.runtime.clone(),
        )?);
        let content_size = window.content_size;
        let hwnd = create_window(
            title.as_str(),
            content_size.width().value(),
            content_size.height().value(),
        )?;
        let viewport = viewport_for_window(hwnd)?;
        if let Err(error) = driver.attach(hwnd, viewport) {
            // SAFETY: This HWND was created on the current UI thread and is not retained yet.
            if let Err(cleanup_error) = unsafe { DestroyWindow(hwnd) } {
                tracing::warn!(%cleanup_error, "failed to destroy window after renderer initialization failed");
            }
            return Err(error);
        }
        self.ids_by_hwnd.borrow_mut().insert(hwnd.0 as isize, id);
        self.windows.borrow_mut().insert(
            id,
            WindowEntry {
                hwnd,
                driver: driver.clone(),
                handle: handle.clone(),
                frame_request_queued: Cell::new(false),
                frame_timer_armed: Cell::new(false),
                mouse_leave_tracked: Cell::new(false),
            },
        );
        // SAFETY: The fully attached top-level HWND is ready to be shown.
        let _previously_visible = unsafe { ShowWindow(hwnd, SW_SHOW) };
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
        let (hwnd, handle) = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| (entry.hwnd, entry.handle.clone()))
            .ok_or(WindowError::Closed(id))?;
        let next_title = match update.title_update() {
            PropertyUpdate::Keep => None,
            PropertyUpdate::Set(title) => {
                set_window_title(hwnd, title.as_str())?;
                Some(title.clone())
            }
            PropertyUpdate::Reset => {
                set_window_title(hwnd, self.name.as_str())?;
                Some(self.name.clone())
            }
        };
        let next_size = match update.content_size_update() {
            PropertyUpdate::Keep => None,
            PropertyUpdate::Set(size) => {
                set_window_content_size(hwnd, *size)?;
                Some(*size)
            }
            PropertyUpdate::Reset => {
                let size = WindowSize::default();
                set_window_content_size(hwnd, size)?;
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
        self.schedule_dirty_windows();
        self.sync_window_info(id);
        Ok(())
    }

    fn apply_action(&self, id: WindowId, action: WindowAction) -> Result<(), AppError> {
        let hwnd = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| entry.hwnd)
            .ok_or(WindowError::Closed(id))?;
        match action {
            WindowAction::Focus => {
                // SAFETY: Requests activation for this live top-level HWND.
                let _focused = unsafe { SetForegroundWindow(hwnd) };
            }
            WindowAction::Minimize => {
                // SAFETY: Changes show state for this live top-level HWND.
                let _previous = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
            }
            WindowAction::Maximize => {
                // SAFETY: Changes show state for this live top-level HWND.
                let _previous = unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
            }
            WindowAction::Restore => {
                // SAFETY: Changes show state for this live top-level HWND.
                let _previous = unsafe { ShowWindow(hwnd, SW_RESTORE) };
            }
            WindowAction::Close => {
                // SAFETY: Queues an ordinary close request for this live HWND.
                unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
                    .map_err(win32_error)?;
            }
        }
        Ok(())
    }

    fn request_display(&self, id: WindowId) {
        let hwnd = {
            let windows = self.windows.borrow();
            let Some(entry) = windows.get(&id) else {
                return;
            };
            if entry.frame_request_queued.replace(true) {
                return;
            }
            entry.hwnd
        };
        // SAFETY: Queues a private frame request for this live HWND.
        if let Err(error) =
            unsafe { PostMessageW(Some(hwnd), WM_ANMIXIU_REQUEST_FRAME, WPARAM(0), LPARAM(0)) }
        {
            if let Some(entry) = self.windows.borrow().get(&id) {
                entry.frame_request_queued.set(false);
            }
            self.fail(win32_error(error));
        }
    }

    fn id_for_hwnd(&self, hwnd: HWND) -> Option<WindowId> {
        self.ids_by_hwnd.borrow().get(&(hwnd.0 as isize)).copied()
    }

    fn driver_for_hwnd(&self, hwnd: HWND) -> Option<Rc<dyn NativeDriver>> {
        let id = self.id_for_hwnd(hwnd)?;
        self.windows
            .borrow()
            .get(&id)
            .map(|entry| entry.driver.clone())
    }

    fn frame_request_delivered(&self, hwnd: HWND) {
        if let Some(id) = self.id_for_hwnd(hwnd)
            && let Some(entry) = self.windows.borrow().get(&id)
        {
            entry.frame_request_queued.set(false);
        }
    }

    fn arm_frame_timer(&self, hwnd: HWND) -> bool {
        let Some(id) = self.id_for_hwnd(hwnd) else {
            return false;
        };
        let windows = self.windows.borrow();
        let Some(entry) = windows.get(&id) else {
            return false;
        };
        !entry.frame_timer_armed.replace(true)
    }

    fn frame_timer_fired(&self, hwnd: HWND) {
        if let Some(id) = self.id_for_hwnd(hwnd)
            && let Some(entry) = self.windows.borrow().get(&id)
        {
            entry.frame_timer_armed.set(false);
        }
    }

    fn begin_mouse_tracking(&self, hwnd: HWND) -> bool {
        let Some(id) = self.id_for_hwnd(hwnd) else {
            return false;
        };
        let windows = self.windows.borrow();
        let Some(entry) = windows.get(&id) else {
            return false;
        };
        !entry.mouse_leave_tracked.replace(true)
    }

    fn end_mouse_tracking(&self, hwnd: HWND) {
        if let Some(id) = self.id_for_hwnd(hwnd)
            && let Some(entry) = self.windows.borrow().get(&id)
        {
            entry.mouse_leave_tracked.set(false);
        }
    }

    fn window_resized(&self, hwnd: HWND, size_state: usize) {
        let Some(id) = self.id_for_hwnd(hwnd) else {
            return;
        };
        let Some((driver, handle)) = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| (entry.driver.clone(), entry.handle.clone()))
        else {
            return;
        };
        let mut info = handle.info();
        info.visibility = if size_state as u32 == SIZE_MINIMIZED {
            WindowVisibility::Minimized
        } else {
            WindowVisibility::Visible
        };
        info.mode = if size_state as u32 == SIZE_MAXIMIZED {
            WindowMode::Maximized
        } else {
            WindowMode::Windowed
        };
        if info.visibility != WindowVisibility::Minimized
            && let Ok(viewport) = viewport_for_window(hwnd)
        {
            info.content_size =
                WindowSize::new(viewport.logical_size().0, viewport.logical_size().1);
            info.scale_factor = viewport.scale();
            driver.resize(viewport);
        }
        handle.replace_info(info);
        self.schedule_dirty_windows();
    }

    fn window_focused(&self, hwnd: HWND, focused: bool) {
        let Some(id) = self.id_for_hwnd(hwnd) else {
            return;
        };
        if focused {
            self.active_window.set(Some(id));
        } else if self.active_window.get() == Some(id) {
            self.active_window.set(None);
        }
        if let Some(handle) = self.handles.borrow().get(&id) {
            let mut info = handle.info();
            info.focused = focused;
            handle.replace_info(info);
        }
        self.schedule_dirty_windows();
    }

    fn sync_window_info(&self, id: WindowId) {
        let Some((hwnd, handle)) = self
            .windows
            .borrow()
            .get(&id)
            .map(|entry| (entry.hwnd, entry.handle.clone()))
        else {
            return;
        };
        if let Ok(viewport) = viewport_for_window(hwnd) {
            let mut info = handle.info();
            info.content_size =
                WindowSize::new(viewport.logical_size().0, viewport.logical_size().1);
            info.scale_factor = viewport.scale();
            info.status = WindowStatus::Open;
            handle.replace_info(info);
        }
    }

    fn window_destroyed(&self, hwnd: HWND) {
        let Some(id) = self.ids_by_hwnd.borrow_mut().remove(&(hwnd.0 as isize)) else {
            return;
        };
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
        if self.windows.borrow().is_empty() && !self.pending.borrow().is_empty() {
            self.drain_commands();
        }
        if self.windows.borrow().is_empty() {
            // SAFETY: The last live top-level window on this UI thread has closed.
            unsafe { PostQuitMessage(0) };
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
        // SAFETY: Failures reach this boundary on the HWND-owning UI thread.
        unsafe { PostQuitMessage(1) };
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
        let windows: Vec<_> = self
            .windows
            .borrow()
            .values()
            .map(|entry| entry.hwnd)
            .collect();
        for hwnd in windows {
            // SAFETY: Each HWND belongs to this UI thread and is still registered as live.
            if let Err(error) = unsafe { DestroyWindow(hwnd) } {
                self.record_error(win32_error(error));
            }
        }
        self.pending.borrow_mut().clear();
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
    fn attach(&self, hwnd: HWND, viewport: Viewport) -> Result<(), AppError>;
    fn draw(&self);
    fn redraw(&self);
    fn resize(&self, viewport: Viewport);
    fn pointer_moved(&self, point: Point);
    fn pointer_position(&self) -> Point;
    fn cursor_style_at(&self, point: Point) -> CursorStyle;
    fn pointer_exited(&self);
    fn pointer_down(&self, point: Point);
    fn pointer_up(&self, point: Point);
    fn scroll(&self, point: Point, delta_x: f32, delta_y: f32);
    fn system_settings_changed(&self);
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
    frame_builder: FrameBuilder,
    renderer: Option<D3d11Renderer>,
    viewport: Viewport,
    frame: Option<BuiltFrame>,
    pointer: PointerTracker,
    pressed_element: Option<GlobalElementId>,
    needs_frame: bool,
    invalidation_streak: usize,
    stalled: HashSet<OwnerId>,
    last_draw_at: Option<Instant>,
    error: Option<AppError>,
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
        font: FontSpec,
        app_events: AppEvents,
        app_handle: AppHandle,
        window_handle: WindowHandle,
        runtime: Rc<AppRuntime>,
    ) -> Result<Self, AppError> {
        let window_id = window_handle.id();
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
        Ok(Self {
            state: RefCell::new(DriverState {
                window_id,
                runtime,
                owners,
                owner: mounted.owner,
                host: mounted.host,
                frame_builder: FrameBuilder::new_with_font(font)?,
                renderer: None,
                viewport: Viewport::new(1.0, 1.0, 1.0),
                frame: None,
                pointer: PointerTracker::default(),
                pressed_element: None,
                needs_frame: true,
                invalidation_streak: 0,
                stalled: HashSet::new(),
                last_draw_at: None,
                error: None,
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
        // SAFETY: Failure is reported on the UI thread; this terminates its current message loop.
        unsafe { PostQuitMessage(1) };
    }

    fn take_error(&self) -> Option<AppError> {
        self.state.borrow_mut().error.take()
    }

    fn surface_size(viewport: Viewport) -> SurfaceSize {
        let (width, height) = viewport.physical_size();
        SurfaceSize::new(width.max(1), height.max(1)).expect("clamped dimensions are non-zero")
    }

    fn request_display(&self) {
        request_display(self.state.borrow().window_id);
    }
}

impl NativeDriver for ComponentDriver {
    fn attach(&self, hwnd: HWND, viewport: Viewport) -> Result<(), AppError> {
        let renderer = D3d11Renderer::new(hwnd, Self::surface_size(viewport), viewport.scale())?;
        let mut state = self.state.borrow_mut();
        state.viewport = viewport;
        state.renderer = Some(renderer);
        state.needs_frame = true;
        Ok(())
    }

    fn system_settings_changed(&self) {
        let refresh = (|| -> Result<bool, FrameBuildError> {
            let mut state = self.state.borrow_mut();
            if state.error.is_some() {
                return Ok(false);
            }
            let changed = state.frame_builder.refresh_system_text_settings()?;
            if changed {
                state.needs_frame = true;
            }
            Ok(changed)
        })();
        match refresh {
            Ok(true) => self.request_display(),
            Ok(false) => {}
            Err(error) => self.fail(error.into()),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn draw(&self) {
        let mut needs_follow_up = false;
        let result = (|| -> Result<(), AppError> {
            let mut state = self.state.borrow_mut();
            if state.error.is_some() || state.renderer.is_none() {
                return Ok(());
            }
            let now = Instant::now();
            let delta_seconds = state
                .last_draw_at
                .replace(now)
                .map_or(1.0 / 60.0, |previous| {
                    previous.elapsed().as_secs_f32().clamp(1.0 / 240.0, 0.1)
                });
            if state
                .frame
                .as_ref()
                .is_some_and(|frame| frame.advance_scroll(delta_seconds))
            {
                state.frame_builder.note_scrolled();
                state.needs_frame = true;
                needs_follow_up = true;
            }
            let dirty = state.owners.take_dirty();
            for owner in &dirty {
                if state.stalled.remove(owner) {
                    state.invalidation_streak = 0;
                }
            }
            let renderable_dirty = dirty
                .iter()
                .copied()
                .filter(|owner| !state.stalled.contains(owner) && state.host.contains_owner(*owner))
                .collect::<Vec<_>>();
            let rerender = state.frame.is_none() || !renderable_dirty.is_empty();
            if rerender {
                if state.frame.is_none() {
                    state
                        .host
                        .render()
                        .map_err(|error| AppError::Component(error.to_string()))?;
                } else {
                    state
                        .host
                        .render_dirty(&renderable_dirty)
                        .map_err(|error| AppError::Component(error.to_string()))?;
                }
                state.needs_frame = true;
            }
            if !state.needs_frame {
                return Ok(());
            }
            let element = state
                .host
                .element_snapshot()
                .expect("a requested frame has a rendered root");
            let (logical_width, logical_height) = state.viewport.logical_size();
            let scale = state.viewport.scale();
            let mut frame = state.frame_builder.build(
                element.as_ref(),
                Size::new(logical_width, logical_height),
                scale,
            )?;
            let hover_point = state.pointer.is_inside().then(|| {
                let (x, y) = state.pointer.position();
                Point::new(x, y)
            });
            if state.frame_builder.update_hover(&frame, hover_point) {
                frame = state.frame_builder.build(
                    element.as_ref(),
                    Size::new(logical_width, logical_height),
                    scale,
                )?;
            }
            let surface = Self::surface_size(state.viewport);
            let renderer = state.renderer.as_mut().expect("checked above");
            let outcome = renderer.render(&frame.scene, surface, scale)?;
            state.frame = Some(frame);
            match outcome {
                FrameOutcome::Presented => {
                    state.needs_frame = false;
                    state.host.did_paint();
                }
                FrameOutcome::SurfaceOutOfDate { .. } => {
                    state.needs_frame = true;
                    needs_follow_up = true;
                }
            }

            let animating_sites = state.owners.take_animating_with_sites();
            let animating = animating_sites
                .iter()
                .map(|(owner, _)| *owner)
                .collect::<Vec<_>>();
            let self_invalidators =
                anonymous_self_invalidators(&state.owners.dirty_snapshot(), &animating);
            match evaluate_runaway_guard(
                &mut state.invalidation_streak,
                !self_invalidators.is_empty(),
            ) {
                GuardDecision::Settled | GuardDecision::Watching => {}
                GuardDecision::Tripped => {
                    #[cfg(debug_assertions)]
                    return Err(AppError::RenderLoop(state.invalidation_streak));
                    #[cfg(not(debug_assertions))]
                    {
                        for owner in &self_invalidators {
                            if !state.owners.clear_dirty(*owner) {
                                tracing::warn!(
                                    ?owner,
                                    "runaway owner was already absent from the dirty queue"
                                );
                            }
                            state.stalled.insert(*owner);
                        }
                        tracing::error!(
                            frozen_owners = self_invalidators.len(),
                            "component invalidated itself every frame without declaring animation"
                        );
                        state.invalidation_streak = 0;
                    }
                }
            }
            if !animating.is_empty() || state.owners.dirty_len() != 0 {
                needs_follow_up = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
            return;
        }
        if needs_follow_up {
            self.request_display();
        }
    }

    fn redraw(&self) {
        self.state.borrow_mut().needs_frame = true;
        self.draw();
    }

    fn resize(&self, viewport: Viewport) {
        let result = (|| -> Result<bool, AppError> {
            let mut state = self.state.borrow_mut();
            if state.viewport == viewport {
                return Ok(false);
            }
            if let Some(renderer) = state.renderer.as_mut() {
                renderer.resize(Self::surface_size(viewport), viewport.scale())?;
            }
            state.viewport = viewport;
            state.needs_frame = true;
            Ok(true)
        })();
        match result {
            Ok(true) => self.draw(),
            Ok(false) => {}
            Err(error) => self.fail(error),
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

    fn pointer_position(&self) -> Point {
        let (x, y) = self.state.borrow().pointer.position();
        Point::new(x, y)
    }

    fn cursor_style_at(&self, point: Point) -> CursorStyle {
        let state = self.state.borrow();
        state.frame.as_ref().map_or(CursorStyle::Default, |frame| {
            frame
                .scene
                .hit_test(point)
                .map_or(CursorStyle::Default, |hit| frame.cursor_style(hit))
        })
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
        let current = target.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.global_id(hit).cloned())
        });
        let _released_target = state.pointer.release(target.map(|hit| hit.0));
        let pressed = state.pressed_element.take();
        let clicked = (pressed.is_some() && pressed == current)
            .then_some(target)
            .flatten();
        let handler = clicked.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.handler(hit).cloned())
        });
        if let Some(handler) = handler
            && let Some(future) = handler.invoke()
        {
            let owner = clicked
                .and_then(|hit| state.frame.as_ref()?.handler_owner(hit))
                .unwrap_or(state.owner);
            if let Err(error) = state.runtime.ui().spawn(&state.owners, owner, future) {
                tracing::error!(%error, ?owner, "async click handler could not be scheduled");
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
        let consumed = state
            .frame
            .as_ref()
            .is_some_and(|frame| frame.scroll_at_axes(point, delta_x, delta_y));
        if consumed {
            state.frame_builder.note_scrolled();
            state.needs_frame = true;
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

fn create_window(title: &str, logical_width: f32, logical_height: f32) -> Result<HWND, AppError> {
    let class_name = wide_null("Anmixiu.Native.Window");
    let title = wide_null(title);
    // SAFETY: Null requests the module containing this executable.
    let module = unsafe { GetModuleHandleW(None) }.map_err(win32_error)?;
    let instance = HINSTANCE(module.0);
    // SAFETY: Loading a predefined cursor returns a process-shared handle.
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(win32_error)?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: The class descriptor and its nul-terminated class name remain valid through
    // registration; the callback has the required system ABI.
    let atom = unsafe { RegisterClassExW(&raw const class) };
    // SAFETY: GetLastError reads thread-local status immediately after failed registration.
    let registration_error = (atom == 0).then(|| unsafe { GetLastError() });
    if registration_error.is_some_and(|error| error != ERROR_CLASS_ALREADY_EXISTS) {
        return Err(AppError::Win32(
            windows::core::Error::from_thread().to_string(),
        ));
    }
    // SAFETY: DPI awareness was selected before this call; GetDpiForSystem has no pointer inputs.
    let dpi = unsafe { GetDpiForSystem() }.max(96);
    let scale = dpi as f32 / 96.0;
    let mut outer = RECT {
        left: 0,
        top: 0,
        right: (logical_width * scale).round() as i32,
        bottom: (logical_height * scale).round() as i32,
    };
    // SAFETY: `outer` is writable and styles match those used for CreateWindowExW below.
    unsafe {
        AdjustWindowRectExForDpi(
            &raw mut outer,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi,
        )
    }
    .map_err(win32_error)?;
    // SAFETY: Class/title strings are nul-terminated, the class is registered, and all optional
    // parent/menu/create pointers are intentionally null for one top-level window.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer.right - outer.left,
            outer.bottom - outer.top,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(win32_error)
}

fn set_window_title(hwnd: HWND, title: &str) -> Result<(), AppError> {
    let title = wide_null(title);
    // SAFETY: The title is nul-terminated and HWND identifies a live window owned by this thread.
    unsafe { SetWindowTextW(hwnd, PCWSTR(title.as_ptr())) }.map_err(win32_error)
}

fn set_window_content_size(hwnd: HWND, size: WindowSize) -> Result<(), AppError> {
    // SAFETY: GetDpiForWindow reads the scale assigned to this live HWND.
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let scale = dpi as f32 / 96.0;
    let mut outer = RECT {
        left: 0,
        top: 0,
        right: (size.width().value() * scale).round() as i32,
        bottom: (size.height().value() * scale).round() as i32,
    };
    // SAFETY: `outer` is writable and styles match this top-level window.
    unsafe {
        AdjustWindowRectExForDpi(
            &raw mut outer,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi,
        )
    }
    .map_err(win32_error)?;
    // SAFETY: Updates only the size of this live HWND and preserves its position and z-order.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            outer.right - outer.left,
            outer.bottom - outer.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
    .map_err(win32_error)
}

fn run_message_loop() -> Result<(), AppError> {
    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is a valid writable MSG and this thread owns the application loop.
        let status = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if status.0 == -1 {
            return Err(AppError::Win32(
                windows::core::Error::from_thread().to_string(),
            ));
        }
        if status.0 == 0 {
            return Ok(());
        }
        if message.hwnd.0.is_null() && message.message == WM_ANMIXIU_WAKE {
            with_app_session(AppSession::wake);
            continue;
        }
        // SAFETY: The message came from GetMessageW and remains valid for translation/dispatch.
        unsafe {
            let _translated = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

#[allow(clippy::too_many_lines)]
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_ANMIXIU_WAKE => {
            with_app_session(AppSession::wake);
            LRESULT(0)
        }
        WM_ANMIXIU_REQUEST_FRAME => {
            with_app_session(|session| session.frame_request_delivered(hwnd));
            arm_frame_timer(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == FRAME_TIMER_ID => {
            // SAFETY: This timer id was installed for this HWND by arm_frame_timer.
            if let Err(error) = unsafe { KillTimer(Some(hwnd), FRAME_TIMER_ID) } {
                report_win32_error("KillTimer failed", &error);
                return LRESULT(0);
            }
            with_app_session(|session| session.frame_timer_fired(hwnd));
            with_driver(hwnd, |driver| driver.draw());
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: WM_PAINT provides a valid update region; BeginPaint/EndPaint are balanced.
            let _paint_context = unsafe { BeginPaint(hwnd, &raw mut paint) };
            // SAFETY: Balances BeginPaint for the same HWND and PAINTSTRUCT.
            let paint_ended = unsafe { EndPaint(hwnd, &raw const paint) };
            if !paint_ended.as_bool() {
                report_win32_error("EndPaint failed", &windows::core::Error::from_thread());
                return LRESULT(0);
            }
            with_driver(hwnd, |driver| driver.redraw());
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_SIZE => {
            with_app_session(|session| session.window_resized(hwnd, wparam.0));
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            if lparam.0 != 0 {
                // SAFETY: WM_DPICHANGED lParam points to a suggested RECT valid for the callback.
                let suggested = unsafe { &*(lparam.0 as *const RECT) };
                // SAFETY: Suggested bounds come from Windows; flags retain z-order and activation.
                let moved = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
                if let Err(error) = moved {
                    report_win32_error("SetWindowPos failed during a DPI transition", &error);
                    return LRESULT(0);
                }
            }
            if let Ok(viewport) = viewport_for_window(hwnd) {
                with_driver(hwnd, |driver| driver.resize(viewport));
                with_app_session(|session| {
                    if let Some(id) = session.id_for_hwnd(hwnd) {
                        session.sync_window_info(id);
                    }
                });
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            with_driver(hwnd, |driver| driver.system_settings_changed());
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            with_app_session(|session| session.window_focused(hwnd, low_word(wparam.0) != 0));
            // SAFETY: Activation bookkeeping is complete; default processing maintains focus.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_MOUSEMOVE => {
            let should_track = ACTIVE_APP.with(|active| {
                active
                    .borrow()
                    .as_ref()
                    .is_some_and(|session| session.begin_mouse_tracking(hwnd))
            });
            if should_track {
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or(u32::MAX),
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: Tracking struct is writable and identifies the live window.
                if let Err(error) = unsafe { TrackMouseEvent(&raw mut tracking) } {
                    with_app_session(|session| session.end_mouse_tracking(hwnd));
                    report_win32_error("TrackMouseEvent failed", &error);
                    return LRESULT(0);
                }
            }
            let point = logical_client_point(hwnd, signed_low(lparam.0), signed_high(lparam.0));
            with_driver(hwnd, |driver| {
                driver.pointer_moved(point);
                set_cursor(driver.cursor_style_at(point));
            });
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_MOUSELEAVE_MESSAGE => {
            with_app_session(|session| session.end_mouse_tracking(hwnd));
            with_driver(hwnd, |driver| driver.pointer_exited());
            with_app_session(AppSession::drain_commands_if_idle);
            set_cursor(CursorStyle::Default);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // SAFETY: Capturing routes the matching button-up to this live window.
            let _previous_capture = unsafe { SetCapture(hwnd) };
            let point = logical_client_point(hwnd, signed_low(lparam.0), signed_high(lparam.0));
            with_driver(hwnd, |driver| driver.pointer_down(point));
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: Releases capture previously acquired by the button-down handler.
            if let Err(error) = unsafe { ReleaseCapture() } {
                tracing::debug!(%error, "pointer capture had already moved before button release");
            }
            let point = logical_client_point(hwnd, signed_low(lparam.0), signed_high(lparam.0));
            with_driver(hwnd, |driver| driver.pointer_up(point));
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let mut screen = POINT {
                x: signed_low(lparam.0),
                y: signed_high(lparam.0),
            };
            // SAFETY: `screen` is writable and the HWND is the wheel target.
            let converted = unsafe { ScreenToClient(hwnd, &raw mut screen) };
            if !converted.as_bool() {
                report_win32_error(
                    "ScreenToClient failed for a wheel event",
                    &windows::core::Error::from_thread(),
                );
                return LRESULT(0);
            }
            let point = logical_client_point(hwnd, screen.x, screen.y);
            let delta = f32::from(high_word(wparam.0).cast_signed()) / 120.0 * 48.0;
            let shift = wparam.0 & SHIFT_BUTTON_MASK != 0;
            let (delta_x, delta_y) = if message == WM_MOUSEHWHEEL || shift {
                (delta, 0.0)
            } else {
                (0.0, -delta)
            };
            with_driver(hwnd, |driver| driver.scroll(point, delta_x, delta_y));
            with_app_session(AppSession::drain_commands_if_idle);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if u32::from(low_word(lparam.0 as usize)) != HTCLIENT {
                // SAFETY: Non-client cursors (resize borders, title bar) belong to Win32.
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            let style = ACTIVE_APP.with(|active| {
                active
                    .borrow()
                    .as_ref()
                    .and_then(|session| session.driver_for_hwnd(hwnd))
                    .map(|driver| driver.cursor_style_at(driver.pointer_position()))
            });
            if let Some(style) = style {
                set_cursor(style);
                return LRESULT(1);
            }
            // SAFETY: Unhandled cursor messages follow Win32 default processing.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CLOSE => {
            // SAFETY: The user requested closing this live top-level window.
            if let Err(error) = unsafe { DestroyWindow(hwnd) } {
                report_win32_error("DestroyWindow failed", &error);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            with_app_session(|session| session.window_destroyed(hwnd));
            LRESULT(0)
        }
        _ => {
            // SAFETY: Every unhandled message is delegated to the system default window proc.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

fn arm_frame_timer(hwnd: HWND) {
    let should_arm = ACTIVE_APP.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_some_and(|session| session.arm_frame_timer(hwnd))
    });
    if !should_arm {
        return;
    }
    // SAFETY: Installs a window-owned timer; no callback pointer means delivery through WM_TIMER.
    let timer = unsafe { SetTimer(Some(hwnd), FRAME_TIMER_ID, FRAME_INTERVAL_MILLIS, None) };
    if timer == 0 {
        with_app_session(|session| session.frame_timer_fired(hwnd));
        with_driver(hwnd, |driver| driver.draw());
    }
}

fn viewport_for_window(hwnd: HWND) -> Result<Viewport, AppError> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid writable out parameter for the live HWND.
    unsafe { GetClientRect(hwnd, &raw mut rect) }.map_err(win32_error)?;
    // SAFETY: GetDpiForWindow reads the scale assigned to this live HWND.
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let scale = dpi as f32 / 96.0;
    let physical_width = u32::try_from((rect.right - rect.left).max(1)).unwrap_or(1);
    let physical_height = u32::try_from((rect.bottom - rect.top).max(1)).unwrap_or(1);
    Ok(Viewport::with_backing_size(
        physical_width as f32 / scale,
        physical_height as f32 / scale,
        scale,
        physical_width,
        physical_height,
    ))
}

fn logical_client_point(hwnd: HWND, x: i32, y: i32) -> Point {
    // SAFETY: GetDpiForWindow reads the current scale of the live input target.
    let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
    Point::new(x as f32 / scale, y as f32 / scale)
}

fn set_cursor(style: CursorStyle) {
    let resource = match style {
        CursorStyle::Default => IDC_ARROW,
        CursorStyle::Pointer => IDC_HAND,
        CursorStyle::Text => IDC_IBEAM,
    };
    // SAFETY: Predefined cursor resources are process-shared and remain valid after loading.
    if let Ok(cursor) = unsafe { LoadCursorW(None, resource) } {
        // SAFETY: Sets a process-shared cursor handle for the current UI thread.
        unsafe { SetCursor(Some(cursor)) };
    }
}

fn with_driver(hwnd: HWND, operation: impl FnOnce(&Rc<dyn NativeDriver>)) {
    let driver = ACTIVE_APP.with(|active| {
        active
            .borrow()
            .as_ref()
            .and_then(|session| session.driver_for_hwnd(hwnd))
    });
    if let Some(driver) = driver {
        operation(&driver);
        with_app_session(AppSession::schedule_dirty_windows);
    }
}

fn with_app_session(operation: impl FnOnce(&AppSession)) {
    ACTIVE_APP.with(|active| {
        if let Some(session) = active.borrow().as_ref() {
            operation(session);
        }
    });
}

fn report_win32_error(operation: &'static str, error: &windows::core::Error) {
    let message = format!("{operation}: {error}");
    let reported = ACTIVE_APP.with(|active| {
        let active = active.borrow();
        let Some(session) = active.as_ref() else {
            return false;
        };
        session.fail(AppError::Win32(message.clone()));
        true
    });
    if !reported {
        tracing::error!(%error, operation, "Win32 operation failed without an active application driver");
    }
}

#[allow(clippy::needless_pass_by_value)]
fn win32_error(error: windows::core::Error) -> AppError {
    AppError::Win32(error.to_string())
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn signed_low(value: isize) -> i32 {
    i32::from((value as u16).cast_signed())
}

fn signed_high(value: isize) -> i32 {
    i32::from(((value as usize >> 16) as u16).cast_signed())
}

fn high_word(value: usize) -> u16 {
    (value >> 16) as u16
}

fn low_word(value: usize) -> u16 {
    value as u16
}

fn anonymous_self_invalidators(dirty: &[OwnerId], animating: &[OwnerId]) -> Vec<OwnerId> {
    let animating = animating.iter().copied().collect::<HashSet<_>>();
    dirty
        .iter()
        .copied()
        .filter(|owner| !animating.contains(owner))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuardDecision {
    Settled,
    Watching,
    Tripped,
}

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

#[cfg(test)]
mod tests {
    use super::{
        GuardDecision, MAX_RENDER_INVALIDATIONS, anonymous_self_invalidators,
        evaluate_runaway_guard, high_word, low_word, signed_high, signed_low,
    };
    use anmixiu_reactive::OwnerRegistry;

    #[test]
    fn signed_message_coordinates_preserve_negative_monitor_positions() {
        let packed = ((-12_i16 as u16 as usize) << 16) | (-3_i16 as u16 as usize);
        assert_eq!(signed_low(packed.cast_signed()), -3);
        assert_eq!(signed_high(packed.cast_signed()), -12);
        assert_eq!(high_word(packed), (-12_i16) as u16);
        assert_eq!(low_word(packed), (-3_i16) as u16);
    }

    #[test]
    fn declared_animation_is_excluded_from_the_render_loop_guard() {
        let owners = OwnerRegistry::new();
        let owner = owners.create_owner();
        assert!(anonymous_self_invalidators(&[owner], &[owner]).is_empty());
        let mut streak = MAX_RENDER_INVALIDATIONS - 1;
        assert_eq!(
            evaluate_runaway_guard(&mut streak, false),
            GuardDecision::Settled
        );
        assert_eq!(streak, 0);
    }
}
