#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

mod driver;

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{HashMap, VecDeque},
    rc::{Rc, Weak},
    sync::Arc,
};

use crate::{FrameBuildError, Viewport};
use anmixiu_core::{
    AppEvents, AppHandle, AppStateStore, Element, Pixels, PropertyUpdate, SharedString, Typography,
    Window, WindowAction, WindowDispatcher, WindowError, WindowHandle, WindowId as CoreWindowId,
    WindowInfo, WindowMode, WindowRoot, WindowSize, WindowStatus, WindowUpdate, WindowVisibility,
};
use anmixiu_render::RenderError;
use anmixiu_runtime::{AppRuntime, RuntimeBuildError};
use thiserror::Error;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::ModifiersState,
    window::{CursorIcon, Window as NativeWindow, WindowId as NativeWindowId},
};

use driver::ComponentDriver;

use anmixiu_text::FontSpec;

const MAX_PENDING_WINDOW_COMMANDS: usize = 1_024;

#[derive(Clone, Copy, Debug)]
pub(super) enum UserEvent {
    Wake,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("winit event-loop operation failed: {0}")]
    EventLoop(String),
    #[error(transparent)]
    Runtime(#[from] RuntimeBuildError),
    #[error(transparent)]
    Frame(#[from] FrameBuildError),
    #[error(transparent)]
    Wgpu(#[from] RenderError),
    #[error("component invalidated itself for more than {0} consecutive display turns")]
    RenderLoop(usize),
    #[error("UI executor thread-affinity failure: {0}")]
    UiThread(String),
    #[error("Element lifecycle render failed: {0}")]
    Element(String),
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
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.typography = self.typography.with_font_family(family);
        self
    }

    #[must_use]
    pub fn font_size(mut self, size: impl Into<Pixels>) -> Self {
        self.typography = self.typography.with_font_size(size);
        self
    }

    #[must_use]
    pub fn events(&self) -> AppEvents {
        self.events.clone()
    }

    /// Starts the native winit event loop and blocks until the last window closes.
    ///
    /// # Errors
    ///
    /// Returns startup, rendering, or guarded render-loop failures.
    pub fn run<C: Element>(self, root: C) -> Result<(), AppError> {
        self.run_internal(WindowRoot::new(root))
    }

    fn run_internal(self, root: WindowRoot) -> Result<(), AppError> {
        let event_loop = EventLoop::<UserEvent>::with_user_event()
            .build()
            .map_err(|error| AppError::EventLoop(error.to_string()))?;
        let session = Rc::new(AppSession::new(
            self.name,
            self.state,
            self.events,
            self.typography,
            event_loop.create_proxy(),
        )?);
        session.install_dispatcher();
        let _initial_window = session.open_window(self.window, root)?;
        let mut application = WinitApplication {
            session: session.clone(),
        };
        let event_loop_result = event_loop
            .run_app(&mut application)
            .map_err(|error| AppError::EventLoop(error.to_string()));
        session.shutdown();
        if let Some(error) = session.take_error() {
            return Err(error);
        }
        event_loop_result
    }
}

enum WindowCommand {
    Open {
        id: CoreWindowId,
        window: Window,
        root: WindowRoot,
        handle: WindowHandle,
    },
    Update {
        id: CoreWindowId,
        update: WindowUpdate,
    },
    Action {
        id: CoreWindowId,
        action: WindowAction,
    },
}

struct WindowEntry {
    window: Arc<NativeWindow>,
    driver: Rc<ComponentDriver>,
    handle: WindowHandle,
    modifiers: Cell<ModifiersState>,
    occluded: Cell<bool>,
}

struct AppSession {
    name: SharedString,
    state: AppStateStore,
    events: AppEvents,
    typography: Typography,
    runtime: Rc<AppRuntime>,
    proxy: EventLoopProxy<UserEvent>,
    dispatcher: OnceCell<Weak<dyn WindowDispatcher>>,
    next_window_id: Cell<u64>,
    pending: RefCell<VecDeque<WindowCommand>>,
    draining: Cell<bool>,
    windows: RefCell<HashMap<CoreWindowId, WindowEntry>>,
    native_ids: RefCell<HashMap<NativeWindowId, CoreWindowId>>,
    handles: RefCell<HashMap<CoreWindowId, WindowHandle>>,
    active_window: Cell<Option<CoreWindowId>>,
    error: RefCell<Option<AppError>>,
}

impl AppSession {
    fn new(
        name: SharedString,
        state: AppStateStore,
        events: AppEvents,
        typography: Typography,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, AppError> {
        let wake_proxy = proxy.clone();
        let runtime = AppRuntime::new(move || send_wake(&wake_proxy))?;
        Ok(Self {
            name,
            state,
            events,
            typography,
            runtime: Rc::new(runtime),
            proxy,
            dispatcher: OnceCell::new(),
            next_window_id: Cell::new(1),
            pending: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
            windows: RefCell::new(HashMap::new()),
            native_ids: RefCell::new(HashMap::new()),
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
        send_wake(&self.proxy);
        Ok(())
    }

    fn wake(&self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.runtime.ui().run_ready() {
            self.record_error(AppError::UiThread(error.to_string()));
        }
        self.drain_commands(event_loop);
        self.schedule_dirty_windows();
        if self.error.borrow().is_some() {
            event_loop.exit();
        }
    }

    fn drain_commands(&self, event_loop: &ActiveEventLoop) {
        if self.draining.replace(true) {
            return;
        }
        let result = drain_reentrant_queue(&self.pending, |command| match command {
            WindowCommand::Open {
                id,
                window,
                root,
                handle,
            } => self.open_native_window(event_loop, id, window, root, &handle),
            WindowCommand::Update { id, update } => self.apply_update(id, &update),
            WindowCommand::Action { id, action } => self.apply_action(event_loop, id, action),
        });
        self.draining.set(false);
        if let Err(error) = result {
            self.record_error(error);
            event_loop.exit();
        }
    }

    fn open_native_window(
        &self,
        event_loop: &ActiveEventLoop,
        id: CoreWindowId,
        window: Window,
        root: WindowRoot,
        handle: &WindowHandle,
    ) -> Result<(), AppError> {
        let window = window.into_parts();
        let title = window.title.unwrap_or_else(|| self.name.clone());
        let typography = window.typography.with_fallback(&self.typography);
        let content_size = window.content_size;
        let attributes = NativeWindow::default_attributes()
            .with_title(title.as_str())
            .with_inner_size(LogicalSize::new(
                f64::from(content_size.width().value()),
                f64::from(content_size.height().value()),
            ))
            .with_visible(false);
        let native_window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|error| AppError::EventLoop(error.to_string()))?,
        );
        let viewport = viewport_for_window(&native_window);
        let driver = Rc::new(ComponentDriver::new(
            root,
            self.state.clone(),
            window.state,
            font_spec(&typography),
            self.events.clone(),
            self.app_handle(),
            handle.clone(),
            self.runtime.clone(),
            self.proxy.clone(),
            native_window.clone(),
            viewport,
        )?);
        self.native_ids.borrow_mut().insert(native_window.id(), id);
        self.windows.borrow_mut().insert(
            id,
            WindowEntry {
                window: native_window.clone(),
                driver: driver.clone(),
                handle: handle.clone(),
                modifiers: Cell::new(ModifiersState::empty()),
                occluded: Cell::new(false),
            },
        );
        self.active_window.set(Some(id));
        handle.replace_info(window_info(
            id,
            title,
            &native_window,
            false,
            WindowStatus::Open,
        ));
        driver.draw();
        native_window.set_visible(true);
        native_window.request_redraw();
        Ok(())
    }

    fn apply_update(&self, id: CoreWindowId, update: &WindowUpdate) -> Result<(), AppError> {
        let windows = self.windows.borrow();
        let entry = windows.get(&id).ok_or(WindowError::Closed(id))?;
        let next_title = match update.title_update() {
            PropertyUpdate::Keep => None,
            PropertyUpdate::Set(title) => {
                entry.window.set_title(title.as_str());
                Some(title.clone())
            }
            PropertyUpdate::Reset => {
                entry.window.set_title(self.name.as_str());
                Some(self.name.clone())
            }
        };
        let next_size = match update.content_size_update() {
            PropertyUpdate::Keep => None,
            PropertyUpdate::Set(size) => {
                request_inner_size(&entry.window, *size);
                Some(*size)
            }
            PropertyUpdate::Reset => {
                let size = WindowSize::default();
                request_inner_size(&entry.window, size);
                Some(size)
            }
        };
        let handle = entry.handle.clone();
        drop(windows);
        if next_title.is_some() || next_size.is_some() {
            let mut info = handle.info();
            if let Some(title) = next_title {
                info.title = title;
            }
            if let Some(size) = next_size {
                info.content_size = size;
            }
            handle.replace_info(info);
        }
        Ok(())
    }

    fn apply_action(
        &self,
        event_loop: &ActiveEventLoop,
        id: CoreWindowId,
        action: WindowAction,
    ) -> Result<(), AppError> {
        if action == WindowAction::Close {
            self.close_window(event_loop, id);
            return Ok(());
        }
        let windows = self.windows.borrow();
        let entry = windows.get(&id).ok_or(WindowError::Closed(id))?;
        match action {
            WindowAction::Focus => entry.window.focus_window(),
            WindowAction::Minimize => entry.window.set_minimized(true),
            WindowAction::Maximize => entry.window.set_maximized(true),
            WindowAction::Restore => {
                entry.window.set_minimized(false);
                entry.window.set_maximized(false);
            }
            WindowAction::Close => unreachable!("handled above"),
        }
        drop(windows);
        self.sync_window_info(id);
        Ok(())
    }

    fn handle_window_event(
        &self,
        event_loop: &ActiveEventLoop,
        native_id: NativeWindowId,
        event: &WindowEvent,
    ) {
        let Some(id) = self.native_ids.borrow().get(&native_id).copied() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                self.close_window(event_loop, id);
            }
            WindowEvent::Resized(size) => self.resize_window(id, *size),
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(size) = self
                    .windows
                    .borrow()
                    .get(&id)
                    .map(|entry| entry.window.inner_size())
                {
                    self.resize_window(id, size);
                }
            }
            WindowEvent::RedrawRequested => self.redraw_window(id),
            WindowEvent::Focused(focused) => self.focus_window(id, *focused),
            WindowEvent::Occluded(occluded) => self.set_occluded(id, *occluded),
            WindowEvent::CursorMoved { position, .. } => self.pointer_moved(id, *position),
            WindowEvent::CursorLeft { .. } => self.pointer_exited(id),
            WindowEvent::MouseInput { state, button, .. } => {
                self.pointer_button(id, *state, *button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.scroll(id, *delta),
            WindowEvent::ModifiersChanged(modifiers) => {
                if let Some(entry) = self.windows.borrow().get(&id) {
                    entry.modifiers.set(modifiers.state());
                }
            }
            _ => {}
        }
        if self.error.borrow().is_some() {
            event_loop.exit();
        }
    }

    fn resize_window(&self, id: CoreWindowId, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            self.sync_window_info(id);
            return;
        }
        if let Some(entry) = self.windows.borrow().get(&id) {
            entry
                .driver
                .resize(viewport_from_physical(size, entry.window.scale_factor()));
            entry.window.request_redraw();
        }
        self.sync_window_info(id);
    }

    fn redraw_window(&self, id: CoreWindowId) {
        if let Some(entry) = self.windows.borrow().get(&id) {
            if !entry.occluded.get() {
                entry.window.pre_present_notify();
                entry.driver.draw();
            }
            if entry.driver.is_dirty() && !entry.occluded.get() {
                entry.window.request_redraw();
            }
            if let Some(error) = entry.driver.take_error() {
                self.record_error(error);
            }
        }
    }

    fn focus_window(&self, id: CoreWindowId, focused: bool) {
        if focused {
            self.active_window.set(Some(id));
        } else if self.active_window.get() == Some(id) {
            self.active_window.set(None);
        }
        self.sync_window_info(id);
    }

    fn set_occluded(&self, id: CoreWindowId, occluded: bool) {
        if let Some(entry) = self.windows.borrow().get(&id) {
            entry.occluded.set(occluded);
            if !occluded {
                entry.window.request_redraw();
            }
        }
        self.sync_window_info(id);
    }

    fn pointer_moved(&self, id: CoreWindowId, position: PhysicalPosition<f64>) {
        if let Some(entry) = self.windows.borrow().get(&id) {
            let scale = entry.window.scale_factor();
            let point =
                anmixiu_scene::Point::new((position.x / scale) as f32, (position.y / scale) as f32);
            entry.driver.pointer_moved(point);
            entry
                .window
                .set_cursor(cursor_icon(entry.driver.cursor_style_at(point)));
        }
    }

    fn pointer_exited(&self, id: CoreWindowId) {
        if let Some(entry) = self.windows.borrow().get(&id) {
            entry.driver.pointer_exited();
            entry.window.set_cursor(CursorIcon::Default);
        }
    }

    fn pointer_button(&self, id: CoreWindowId, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        if let Some(entry) = self.windows.borrow().get(&id) {
            let point = entry.driver.pointer_position();
            match state {
                ElementState::Pressed => entry.driver.pointer_down(point),
                ElementState::Released => entry.driver.pointer_up(point),
            }
        }
    }

    fn scroll(&self, id: CoreWindowId, delta: MouseScrollDelta) {
        if let Some(entry) = self.windows.borrow().get(&id) {
            let scale = entry.window.scale_factor() as f32;
            let (mut delta_x, mut delta_y) = match delta {
                MouseScrollDelta::LineDelta(x, y) => (x * 16.0, y * 16.0),
                MouseScrollDelta::PixelDelta(position) => {
                    (position.x as f32 / scale, position.y as f32 / scale)
                }
            };
            if delta_x.abs() <= f32::EPSILON
                && delta_y.abs() > f32::EPSILON
                && entry.modifiers.get().shift_key()
            {
                delta_x = delta_y;
                delta_y = 0.0;
            }
            entry
                .driver
                .scroll(entry.driver.pointer_position(), -delta_x, -delta_y);
        }
    }

    fn schedule_dirty_windows(&self) {
        for entry in self.windows.borrow().values() {
            if entry.driver.is_dirty() && !entry.occluded.get() {
                entry.window.request_redraw();
            }
        }
    }

    fn sync_window_info(&self, id: CoreWindowId) {
        let windows = self.windows.borrow();
        let Some(entry) = windows.get(&id) else {
            return;
        };
        let current = entry.handle.info();
        entry.handle.replace_info(window_info(
            id,
            current.title,
            &entry.window,
            entry.occluded.get(),
            WindowStatus::Open,
        ));
    }

    fn close_window(&self, event_loop: &ActiveEventLoop, id: CoreWindowId) {
        let Some(entry) = self.windows.borrow_mut().remove(&id) else {
            return;
        };
        self.native_ids.borrow_mut().remove(&entry.window.id());
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
            event_loop.exit();
        }
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
        for entry in self.windows.borrow_mut().drain().map(|(_, entry)| entry) {
            entry.driver.shutdown();
            let mut info = entry.handle.info();
            info.status = WindowStatus::Closed;
            info.visibility = WindowVisibility::Hidden;
            info.focused = false;
            entry.handle.replace_info(info);
        }
        self.native_ids.borrow_mut().clear();
        self.handles.borrow_mut().clear();
        self.pending.borrow_mut().clear();
    }
}

impl WindowDispatcher for AppSession {
    fn open_window(&self, window: Window, root: WindowRoot) -> Result<WindowHandle, WindowError> {
        if self.pending.borrow().len() >= MAX_PENDING_WINDOW_COMMANDS {
            return Err(WindowError::CommandQueueFull);
        }
        let raw_id = self.next_window_id.get();
        let next = raw_id.checked_add(1).ok_or(WindowError::IdExhausted)?;
        let id = CoreWindowId::new(raw_id);
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

    fn update_window(&self, id: CoreWindowId, update: WindowUpdate) -> Result<(), WindowError> {
        if !self.handles.borrow().contains_key(&id) {
            return Err(WindowError::Closed(id));
        }
        self.enqueue(WindowCommand::Update { id, update })
    }

    fn window_action(&self, id: CoreWindowId, action: WindowAction) -> Result<(), WindowError> {
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

struct WinitApplication {
    session: Rc<AppSession>,
}

impl ApplicationHandler<UserEvent> for WinitApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.session.wake(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: UserEvent) {
        self.session.wake(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: NativeWindowId,
        event: WindowEvent,
    ) {
        self.session
            .handle_window_event(event_loop, window_id, &event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.session.wake(event_loop);
    }
}

pub(super) fn send_wake(proxy: &EventLoopProxy<UserEvent>) {
    if let Err(error) = proxy.send_event(UserEvent::Wake) {
        tracing::debug!(%error, "winit event loop already stopped before wake delivery");
    }
}

fn drain_reentrant_queue<T, E>(
    queue: &RefCell<VecDeque<T>>,
    mut operation: impl FnMut(T) -> Result<(), E>,
) -> Result<(), E> {
    loop {
        let item = queue.borrow_mut().pop_front();
        let Some(item) = item else {
            return Ok(());
        };
        operation(item)?;
    }
}

fn viewport_for_window(window: &NativeWindow) -> Viewport {
    viewport_from_physical(window.inner_size(), window.scale_factor())
}

fn viewport_from_physical(size: PhysicalSize<u32>, scale: f64) -> Viewport {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale as f32
    } else {
        1.0
    };
    Viewport::with_backing_size(
        size.width as f32 / scale,
        size.height as f32 / scale,
        scale,
        size.width.max(1),
        size.height.max(1),
    )
}

fn request_inner_size(window: &NativeWindow, size: WindowSize) {
    if let Some(size) = window.request_inner_size(LogicalSize::new(
        f64::from(size.width().value()),
        f64::from(size.height().value()),
    )) {
        tracing::trace!(?size, "winit applied requested inner size synchronously");
    }
}

fn window_info(
    id: CoreWindowId,
    title: SharedString,
    window: &NativeWindow,
    occluded: bool,
    status: WindowStatus,
) -> WindowInfo {
    let viewport = viewport_for_window(window);
    let minimized = window.is_minimized().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(true);
    WindowInfo {
        id,
        title,
        content_size: WindowSize::new(
            viewport.logical_size().0.max(f32::EPSILON),
            viewport.logical_size().1.max(f32::EPSILON),
        ),
        scale_factor: viewport.scale(),
        focused: window.has_focus(),
        visibility: if minimized {
            WindowVisibility::Minimized
        } else if !visible || occluded {
            WindowVisibility::Hidden
        } else {
            WindowVisibility::Visible
        },
        mode: if window.fullscreen().is_some() {
            WindowMode::Fullscreen
        } else if window.is_maximized() {
            WindowMode::Maximized
        } else {
            WindowMode::Windowed
        },
        status,
    }
}

fn cursor_icon(style: anmixiu_core::CursorStyle) -> CursorIcon {
    match style {
        anmixiu_core::CursorStyle::Default => CursorIcon::Default,
        anmixiu_core::CursorStyle::Pointer => CursorIcon::Pointer,
        anmixiu_core::CursorStyle::Text => CursorIcon::Text,
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

#[cfg(test)]
mod tests {
    use super::viewport_from_physical;
    use winit::dpi::PhysicalSize;

    #[test]
    fn physical_window_size_and_scale_produce_one_consistent_viewport() {
        let viewport = viewport_from_physical(PhysicalSize::new(1240, 1040), 2.0);
        assert_eq!(viewport.logical_size(), (620.0, 520.0));
        assert_eq!(viewport.physical_size(), (1240, 1040));
        assert!((viewport.scale() - 2.0).abs() <= f32::EPSILON);
    }
}
