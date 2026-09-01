#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::c_void,
    mem::size_of,
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicIsize, Ordering},
    time::Instant,
};

use anmixiu_core::{
    AppStateStore, ComponentHost, Context, CursorStyle, GlobalElementId, Pixels, Render,
    Typography, WindowStateStore,
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
        System::LibraryLoader::GetModuleHandleW,
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
                RegisterClassExW, SIZE_MINIMIZED, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SetCursor,
                SetTimer, SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_APP,
                WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEHWHEEL,
                WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE,
                WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
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

static ACTIVE_HWND: AtomicIsize = AtomicIsize::new(0);
static FRAME_REQUEST_QUEUED: AtomicBool = AtomicBool::new(false);
static FRAME_TIMER_ARMED: AtomicBool = AtomicBool::new(false);
static MOUSE_LEAVE_TRACKED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static ACTIVE_DRIVER: RefCell<Option<Rc<dyn NativeDriver>>> = RefCell::new(None);
}

fn active_hwnd() -> Option<HWND> {
    let raw = ACTIVE_HWND.load(Ordering::Acquire);
    (raw != 0).then_some(HWND(raw as *mut c_void))
}

fn wake_win32() {
    let Some(hwnd) = active_hwnd() else {
        return;
    };
    // SAFETY: `ACTIVE_HWND` is installed only while the window is live. PostMessageW is
    // thread-safe and only queues work; all Rust state remains confined to the UI thread.
    if let Err(error) = unsafe { PostMessageW(Some(hwnd), WM_ANMIXIU_WAKE, WPARAM(0), LPARAM(0)) } {
        tracing::debug!(%error, "Win32 UI wake arrived after its window stopped accepting messages");
    }
}

fn request_display() {
    if FRAME_REQUEST_QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(hwnd) = active_hwnd() else {
        FRAME_REQUEST_QUEUED.store(false, Ordering::Release);
        return;
    };
    // SAFETY: The live HWND is used only to queue a private frame-request message.
    if let Err(error) =
        unsafe { PostMessageW(Some(hwnd), WM_ANMIXIU_REQUEST_FRAME, WPARAM(0), LPARAM(0)) }
    {
        FRAME_REQUEST_QUEUED.store(false, Ordering::Release);
        report_win32_error("PostMessageW failed while requesting a frame", error);
    }
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
}

pub struct Window {
    title: String,
    width: f32,
    height: f32,
    state: WindowStateStore,
    typography: Typography,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            title: "Anmixiu".to_owned(),
            width: 560.0,
            height: 460.0,
            state: WindowStateStore::new(),
            typography: Typography::new(),
        }
    }
}

impl Window {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    #[must_use]
    /// Sets the initial logical client-area size.
    ///
    /// # Panics
    ///
    /// Panics unless both dimensions are finite and greater than zero.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        assert!(width.is_finite() && width > 0.0);
        assert!(height.is_finite() && height > 0.0);
        self.width = width;
        self.height = height;
        self
    }

    #[must_use]
    pub fn with_state<T: 'static>(mut self, state: T) -> Self {
        self.state = self.state.with(state);
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
}

pub struct App {
    state: AppStateStore,
    window: Window,
    typography: Typography,
}

impl Default for App {
    fn default() -> Self {
        Self {
            state: AppStateStore::new(),
            window: Window::new(),
            typography: Typography::new(),
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

    /// Creates one native Win32 window and blocks in its message loop until it closes.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime, window, text, or graphics initialization fails, when a
    /// native frame cannot be built or presented, or when the component exceeds the guarded
    /// self-invalidation limit.
    pub fn run<C: Render>(self, root: C) -> Result<(), AppError> {
        // SAFETY: DPI awareness is selected before creating any HWND. ERROR_ACCESS_DENIED means a
        // manifest or the host process already selected awareness, so continuing is correct.
        let dpi_result =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if let Err(error) = dpi_result
            && error.code() != E_ACCESSDENIED
        {
            return Err(win32_error(error));
        }
        let typography = self.window.typography.with_fallback(&self.typography);
        let driver = Rc::new(ComponentDriver::new(
            root,
            self.state,
            self.window.state,
            font_spec(&typography),
        )?);
        ACTIVE_DRIVER.with(|active| {
            active.replace(Some(driver.clone()));
        });

        let hwnd = match create_window(&self.window.title, self.window.width, self.window.height) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                ACTIVE_DRIVER.with(|active| {
                    active.take();
                });
                return Err(error);
            }
        };
        ACTIVE_HWND.store(hwnd.0 as isize, Ordering::Release);
        let viewport = viewport_for_window(hwnd)?;
        if let Err(error) = driver.attach(hwnd, viewport) {
            // SAFETY: The HWND was created on this thread and has not yet been destroyed.
            if let Err(cleanup_error) = unsafe { DestroyWindow(hwnd) } {
                tracing::warn!(%cleanup_error, "failed to destroy the window after renderer initialization failed");
            }
            ACTIVE_HWND.store(0, Ordering::Release);
            ACTIVE_DRIVER.with(|active| {
                active.take();
            });
            return Err(error);
        }
        // SAFETY: The initialized top-level HWND is ready to be shown.
        let _previously_visible = unsafe { ShowWindow(hwnd, SW_SHOW) };
        driver.draw();
        driver.wake();
        request_display();

        let message_result = run_message_loop();
        driver.shutdown();
        ACTIVE_HWND.store(0, Ordering::Release);
        ACTIVE_DRIVER.with(|active| {
            active.take();
        });
        FRAME_REQUEST_QUEUED.store(false, Ordering::Release);
        FRAME_TIMER_ARMED.store(false, Ordering::Release);
        MOUSE_LEAVE_TRACKED.store(false, Ordering::Release);
        message_result?;
        driver.take_error().map_or(Ok(()), Err)
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
    fn wake(&self);
    fn shutdown(&self);
    fn fail_win32(&self, operation: &'static str, error: windows::core::Error);
}

struct DriverState<C: Render> {
    runtime: AppRuntime,
    owners: OwnerRegistry,
    owner: OwnerId,
    host: ComponentHost<C>,
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

struct ComponentDriver<C: Render> {
    state: RefCell<DriverState<C>>,
}

impl<C: Render> ComponentDriver<C> {
    fn new(
        root: C,
        app_state: AppStateStore,
        window_state: WindowStateStore,
        font: FontSpec,
    ) -> Result<Self, AppError> {
        let runtime = AppRuntime::new(wake_win32)?;
        let owners = OwnerRegistry::new();
        let spawner = runtime.ui().spawner(owners.clone());
        let context = Context::with_owner_spawner(
            app_state,
            window_state,
            owners.clone(),
            move |owner, future| {
                if let Err(error) = spawner.spawn(owner, future) {
                    panic!("Context::spawn failed: {error}");
                }
            },
        );
        let owner = context.owner_id();
        Ok(Self {
            state: RefCell::new(DriverState {
                runtime,
                owners,
                owner,
                host: ComponentHost::new(Rc::new(root), context),
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
}

impl<C: Render> NativeDriver for ComponentDriver<C> {
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
            Ok(true) => request_display(),
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
            state
                .runtime
                .ui()
                .run_ready()
                .map_err(|error| AppError::UiThread(error.to_string()))?;
            let dirty = state.owners.take_dirty();
            for owner in &dirty {
                if state.stalled.remove(owner) {
                    state.invalidation_streak = 0;
                }
            }
            let rerender = !state.stalled.contains(&state.owner)
                && (state.frame.is_none() || dirty.contains(&state.owner));
            if rerender {
                state
                    .host
                    .render()
                    .map_err(|error| AppError::Component(error.to_string()))?;
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
            let frame = state.frame_builder.build(
                element.as_ref(),
                Size::new(logical_width, logical_height),
                scale,
            )?;
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
            request_display();
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
        let previous = state.frame_builder.hovered().cloned();
        let hovered = state
            .frame
            .as_ref()
            .and_then(|frame| frame.scene.hit_test(point))
            .and_then(|hit| {
                state
                    .frame
                    .as_ref()
                    .and_then(|frame| frame.global_id(hit).cloned())
            });
        let changed = state.frame_builder.set_hovered(hovered);
        if changed {
            if let Some(frame) = state.frame.as_ref() {
                if let Some(previous) = previous.as_ref()
                    && let Some(handler) = frame.hover_handler(previous)
                {
                    handler.invoke(false);
                }
                if let Some(hovered) = state.frame_builder.hovered()
                    && let Some(handler) = frame.hover_handler(hovered)
                {
                    handler.invoke(true);
                }
            }
            state.needs_frame = true;
        }
        drop(state);
        if changed {
            request_display();
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
        let previous = state.frame_builder.hovered().cloned();
        let changed = state.frame_builder.set_hovered(None);
        if changed {
            if let Some(previous) = previous.as_ref()
                && let Some(frame) = state.frame.as_ref()
                && let Some(handler) = frame.hover_handler(previous)
            {
                handler.invoke(false);
            }
            state.needs_frame = true;
        }
        drop(state);
        if changed {
            request_display();
        }
    }

    fn pointer_down(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let hit = state
            .frame
            .as_ref()
            .and_then(|frame| frame.scene.hit_test(point));
        let focused = hit.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.global_id(hit).cloned())
        });
        state.pressed_element.clone_from(&focused);
        state.pointer.press(hit.map(|hit| hit.0));
    }

    fn pointer_up(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let target = state
            .frame
            .as_ref()
            .and_then(|frame| frame.scene.hit_test(point));
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
            let owner = state.owner;
            if let Err(error) = state.runtime.ui().spawn(&state.owners, owner, future) {
                panic!("async click handler could not be scheduled: {error}");
            }
        }
        let dirty = state.owners.dirty_len() != 0;
        drop(state);
        if dirty {
            request_display();
        }
    }

    fn scroll(&self, point: Point, delta_x: f32, delta_y: f32) {
        let mut state = self.state.borrow_mut();
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
            request_display();
        }
    }

    fn wake(&self) {
        let dirty = {
            let state = self.state.borrow();
            if let Err(error) = state.runtime.ui().run_ready() {
                drop(state);
                self.fail(AppError::UiThread(error.to_string()));
                return;
            }
            state.owners.dirty_len() != 0
        };
        if dirty {
            request_display();
        }
    }

    fn shutdown(&self) {
        self.state.borrow_mut().host.unmount();
    }

    fn fail_win32(&self, operation: &'static str, error: windows::core::Error) {
        self.fail(AppError::Win32(format!("{operation}: {error}")));
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
            with_driver(|driver| driver.wake());
            LRESULT(0)
        }
        WM_ANMIXIU_REQUEST_FRAME => {
            FRAME_REQUEST_QUEUED.store(false, Ordering::Release);
            arm_frame_timer(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == FRAME_TIMER_ID => {
            // SAFETY: This timer id was installed for this HWND by arm_frame_timer.
            if let Err(error) = unsafe { KillTimer(Some(hwnd), FRAME_TIMER_ID) } {
                report_win32_error("KillTimer failed", error);
                return LRESULT(0);
            }
            FRAME_TIMER_ARMED.store(false, Ordering::Release);
            with_driver(|driver| driver.draw());
            LRESULT(0)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: WM_PAINT provides a valid update region; BeginPaint/EndPaint are balanced.
            let _paint_context = unsafe { BeginPaint(hwnd, &raw mut paint) };
            // SAFETY: Balances BeginPaint for the same HWND and PAINTSTRUCT.
            let paint_ended = unsafe { EndPaint(hwnd, &raw const paint) };
            if !paint_ended.as_bool() {
                report_win32_error("EndPaint failed", windows::core::Error::from_thread());
                return LRESULT(0);
            }
            with_driver(|driver| driver.redraw());
            LRESULT(0)
        }
        WM_SIZE if wparam.0 as u32 != SIZE_MINIMIZED => {
            if let Ok(viewport) = viewport_for_window(hwnd) {
                with_driver(|driver| driver.resize(viewport));
            }
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
                    report_win32_error("SetWindowPos failed during a DPI transition", error);
                    return LRESULT(0);
                }
            }
            if let Ok(viewport) = viewport_for_window(hwnd) {
                with_driver(|driver| driver.resize(viewport));
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            with_driver(|driver| driver.system_settings_changed());
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if !MOUSE_LEAVE_TRACKED.swap(true, Ordering::AcqRel) {
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or(u32::MAX),
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: Tracking struct is writable and identifies the live window.
                if let Err(error) = unsafe { TrackMouseEvent(&raw mut tracking) } {
                    MOUSE_LEAVE_TRACKED.store(false, Ordering::Release);
                    report_win32_error("TrackMouseEvent failed", error);
                    return LRESULT(0);
                }
            }
            let point = logical_client_point(hwnd, signed_low(lparam.0), signed_high(lparam.0));
            with_driver(|driver| {
                driver.pointer_moved(point);
                set_cursor(driver.cursor_style_at(point));
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE_MESSAGE => {
            MOUSE_LEAVE_TRACKED.store(false, Ordering::Release);
            with_driver(|driver| driver.pointer_exited());
            set_cursor(CursorStyle::Default);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // SAFETY: Capturing routes the matching button-up to this live window.
            let _previous_capture = unsafe { SetCapture(hwnd) };
            let point = logical_client_point(hwnd, signed_low(lparam.0), signed_high(lparam.0));
            with_driver(|driver| driver.pointer_down(point));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: Releases capture previously acquired by the button-down handler.
            if let Err(error) = unsafe { ReleaseCapture() } {
                tracing::debug!(%error, "pointer capture had already moved before button release");
            }
            let point = logical_client_point(hwnd, signed_low(lparam.0), signed_high(lparam.0));
            with_driver(|driver| driver.pointer_up(point));
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
                    windows::core::Error::from_thread(),
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
            with_driver(|driver| driver.scroll(point, delta_x, delta_y));
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if u32::from(low_word(lparam.0 as usize)) != HTCLIENT {
                // SAFETY: Non-client cursors (resize borders, title bar) belong to Win32.
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            let style = ACTIVE_DRIVER.with(|active| {
                active
                    .borrow()
                    .as_ref()
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
                report_win32_error("DestroyWindow failed", error);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            with_driver(|driver| driver.shutdown());
            ACTIVE_HWND.store(0, Ordering::Release);
            // SAFETY: Ends the one message loop owned by this window thread.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: Every unhandled message is delegated to the system default window proc.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

fn arm_frame_timer(hwnd: HWND) {
    if FRAME_TIMER_ARMED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: Installs a window-owned timer; no callback pointer means delivery through WM_TIMER.
    let timer = unsafe { SetTimer(Some(hwnd), FRAME_TIMER_ID, FRAME_INTERVAL_MILLIS, None) };
    if timer == 0 {
        FRAME_TIMER_ARMED.store(false, Ordering::Release);
        with_driver(|driver| driver.draw());
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

fn with_driver(operation: impl FnOnce(&Rc<dyn NativeDriver>)) {
    ACTIVE_DRIVER.with(|active| {
        if let Some(driver) = active.borrow().as_ref() {
            operation(driver);
        }
    });
}

fn report_win32_error(operation: &'static str, error: windows::core::Error) {
    let mut pending = Some(error);
    ACTIVE_DRIVER.with(|active| {
        if let Some(driver) = active.borrow().as_ref()
            && let Some(error) = pending.take()
        {
            driver.fail_win32(operation, error);
        }
    });
    if let Some(error) = pending {
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
