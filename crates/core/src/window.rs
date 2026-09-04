use std::{
    fmt,
    rc::{Rc, Weak},
};

use anmixiu_reactive::{OwnerId, OwnerRegistry, ReactiveStats, Signal};
use anmixiu_runtime::UiSpawner;
use thiserror::Error;

use crate::{
    AppEvents, AppStateStore, ComponentHost, Context, ElementNode, EventBindings, Eventful, Pixels,
    Render, RenderError, SharedString, Typography, WindowId, WindowStateStore, px,
};

/// A window content-area size measured in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSize {
    width: Pixels,
    height: Pixels,
}

impl WindowSize {
    /// Creates a positive finite logical content size.
    ///
    /// # Panics
    ///
    /// Panics unless both dimensions are finite and greater than zero.
    #[must_use]
    pub fn new(width: impl Into<Pixels>, height: impl Into<Pixels>) -> Self {
        let width = width.into();
        let height = height.into();
        assert!(width.value().is_finite() && width.value() > 0.0);
        assert!(height.value().is_finite() && height.value() > 0.0);
        Self { width, height }
    }

    #[must_use]
    pub const fn width(self) -> Pixels {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> Pixels {
        self.height
    }
}

impl Default for WindowSize {
    fn default() -> Self {
        Self::new(px(560.0), px(460.0))
    }
}

/// Portable creation settings for one native window.
///
/// An omitted title inherits the application's name. Runtime state is exposed separately through
/// [`WindowInfo`] once a window is opened.
#[derive(Clone)]
pub struct Window {
    title: Option<SharedString>,
    content_size: WindowSize,
    state: WindowStateStore,
    typography: Typography,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            title: None,
            content_size: WindowSize::default(),
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

    /// Sets an explicit window title. An empty string deliberately requests an empty title.
    #[must_use]
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Restores application-name inheritance for this window's title.
    #[must_use]
    pub fn inherit_title(mut self) -> Self {
        self.title = None;
        self
    }

    /// Sets the initial logical window content size.
    ///
    /// # Panics
    ///
    /// Panics unless both dimensions are finite and greater than zero.
    #[must_use]
    pub fn size(mut self, width: impl Into<Pixels>, height: impl Into<Pixels>) -> Self {
        self.content_size = WindowSize::new(width, height);
        self
    }

    #[must_use]
    pub fn with_state<T: 'static>(mut self, state: T) -> Self {
        self.state = self.state.with(state);
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

    /// Returns the explicitly requested title, or `None` when the application name is inherited.
    #[must_use]
    pub const fn requested_title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    #[must_use]
    pub const fn content_size(&self) -> WindowSize {
        self.content_size
    }

    #[doc(hidden)]
    #[must_use]
    pub fn into_parts(self) -> WindowParts {
        WindowParts {
            title: self.title,
            content_size: self.content_size,
            state: self.state,
            typography: self.typography,
        }
    }
}

/// Erased fields consumed by native platform adapters.
#[doc(hidden)]
pub struct WindowParts {
    pub title: Option<SharedString>,
    pub content_size: WindowSize,
    pub state: WindowStateStore,
    pub typography: Typography,
}

/// One field in an incremental native-window update.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PropertyUpdate<T> {
    /// Preserve the current effective value.
    #[default]
    Keep,
    /// Install an explicit value.
    Set(T),
    /// Restore the value inherited from application or platform defaults.
    Reset,
}

/// A batched update for portable native-window properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowUpdate {
    title: PropertyUpdate<SharedString>,
    content_size: PropertyUpdate<WindowSize>,
}

impl WindowUpdate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            title: PropertyUpdate::Keep,
            content_size: PropertyUpdate::Keep,
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = PropertyUpdate::Set(title.into());
        self
    }

    #[must_use]
    pub fn reset_title(mut self) -> Self {
        self.title = PropertyUpdate::Reset;
        self
    }

    #[must_use]
    pub fn content_size(mut self, width: impl Into<Pixels>, height: impl Into<Pixels>) -> Self {
        self.content_size = PropertyUpdate::Set(WindowSize::new(width, height));
        self
    }

    #[must_use]
    pub const fn title_update(&self) -> &PropertyUpdate<SharedString> {
        &self.title
    }

    #[must_use]
    pub const fn content_size_update(&self) -> &PropertyUpdate<WindowSize> {
        &self.content_size
    }
}

/// Lifecycle state of a runtime native window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowStatus {
    Opening,
    Open,
    Closing,
    Closed,
}

/// Whether a native window is currently shown or minimized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowVisibility {
    Hidden,
    Visible,
    Minimized,
}

/// Portable presentation mode of a visible native window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowMode {
    Windowed,
    Maximized,
    Fullscreen,
}

/// Resolved, read-only information about a runtime native window.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: SharedString,
    pub content_size: WindowSize,
    pub scale_factor: f32,
    pub focused: bool,
    pub visibility: WindowVisibility,
    pub mode: WindowMode,
    pub status: WindowStatus,
}

/// Failure to submit an operation to a live native window or application.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum WindowError {
    #[error("the Anmixiu application is no longer running")]
    AppStopped,
    #[error("window {0:?} is no longer open")]
    Closed(WindowId),
    #[error("the bounded native-window command queue is full")]
    CommandQueueFull,
    #[error("the native-window identity space is exhausted")]
    IdExhausted,
}

/// A command with no property payload for one live window.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowAction {
    Focus,
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Platform callback surface retained weakly by public app/window handles.
#[doc(hidden)]
pub trait WindowDispatcher {
    fn open_window(&self, window: Window, root: WindowRoot) -> Result<WindowHandle, WindowError>;

    fn update_window(&self, id: WindowId, update: WindowUpdate) -> Result<(), WindowError>;

    fn window_action(&self, id: WindowId, action: WindowAction) -> Result<(), WindowError>;

    fn windows(&self) -> Vec<WindowHandle>;

    fn active_window(&self) -> Option<WindowHandle>;
}

/// Cloneable, main-thread handle to application-level window operations.
#[derive(Clone)]
pub struct AppHandle {
    dispatcher: Weak<dyn WindowDispatcher>,
}

impl fmt::Debug for AppHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AppHandle(..)")
    }
}

impl AppHandle {
    #[doc(hidden)]
    #[must_use]
    pub fn new(dispatcher: Weak<dyn WindowDispatcher>) -> Self {
        Self { dispatcher }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn disconnected() -> Self {
        Self {
            dispatcher: Weak::<DisconnectedWindowDispatcher>::new(),
        }
    }

    /// Opens a native window whose persistent root implements [`Render`].
    ///
    /// # Errors
    ///
    /// Returns an error if the application has stopped or the bounded command queue cannot accept
    /// the operation. Native creation is performed by the UI event loop; a later platform failure
    /// is returned from `App::run` as the platform's `AppError`.
    pub fn open_window<C: Render>(
        &self,
        window: Window,
        root: C,
    ) -> Result<WindowHandle, WindowError> {
        self.dispatcher()?
            .open_window(window, WindowRoot::new(root))
    }

    /// Opens a native window and enables its root's optional [`Eventful`] capability.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`open_window`](Self::open_window).
    pub fn open_eventful_window<C: Render + Eventful>(
        &self,
        window: Window,
        root: C,
    ) -> Result<WindowHandle, WindowError> {
        self.dispatcher()?
            .open_window(window, WindowRoot::new_eventful(root))
    }

    /// Returns handles for all currently opening or open windows.
    #[must_use]
    pub fn windows(&self) -> Vec<WindowHandle> {
        self.dispatcher
            .upgrade()
            .map_or_else(Vec::new, |dispatcher| dispatcher.windows())
    }

    /// Returns the system's currently focused/key window, if it belongs to this application.
    #[must_use]
    pub fn active_window(&self) -> Option<WindowHandle> {
        self.dispatcher
            .upgrade()
            .and_then(|dispatcher| dispatcher.active_window())
    }

    fn dispatcher(&self) -> Result<Rc<dyn WindowDispatcher>, WindowError> {
        self.dispatcher.upgrade().ok_or(WindowError::AppStopped)
    }
}

/// Cloneable, main-thread handle bound to one runtime native window.
#[derive(Clone)]
pub struct WindowHandle {
    id: WindowId,
    dispatcher: Weak<dyn WindowDispatcher>,
    info: Signal<WindowInfo>,
}

impl fmt::Debug for WindowHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowHandle")
            .field("id", &self.id)
            .field("info", &self.info.get())
            .finish_non_exhaustive()
    }
}

impl WindowHandle {
    #[doc(hidden)]
    #[must_use]
    pub fn new(id: WindowId, dispatcher: Weak<dyn WindowDispatcher>, info: WindowInfo) -> Self {
        Self {
            id,
            dispatcher,
            info: Signal::new(info),
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn disconnected(id: WindowId) -> Self {
        Self::new(
            id,
            Weak::<DisconnectedWindowDispatcher>::new(),
            WindowInfo {
                id,
                title: SharedString::new_static("Anmixiu"),
                content_size: WindowSize::default(),
                scale_factor: 1.0,
                focused: false,
                visibility: WindowVisibility::Hidden,
                mode: WindowMode::Windowed,
                status: WindowStatus::Open,
            },
        )
    }

    #[must_use]
    pub const fn id(&self) -> WindowId {
        self.id
    }

    /// Returns the latest native snapshot and subscribes the current render observer to changes.
    #[must_use]
    pub fn info(&self) -> WindowInfo {
        self.info.get()
    }

    /// Applies a batched portable property update.
    ///
    /// # Errors
    ///
    /// Returns an error when this window or application is no longer live, or when the bounded
    /// command queue is full.
    pub fn update(&self, update: WindowUpdate) -> Result<(), WindowError> {
        self.dispatcher()?.update_window(self.id, update)
    }

    /// Requests system focus for this window.
    ///
    /// # Errors
    ///
    /// Returns an error when this window or application is no longer live, or when the bounded
    /// command queue is full.
    pub fn focus(&self) -> Result<(), WindowError> {
        self.action(WindowAction::Focus)
    }

    /// Minimizes this window through the native window manager.
    ///
    /// # Errors
    ///
    /// Returns an error when this window or application is no longer live, or when the bounded
    /// command queue is full.
    pub fn minimize(&self) -> Result<(), WindowError> {
        self.action(WindowAction::Minimize)
    }

    /// Maximizes this window through the native window manager.
    ///
    /// # Errors
    ///
    /// Returns an error when this window or application is no longer live, or when the bounded
    /// command queue is full.
    pub fn maximize(&self) -> Result<(), WindowError> {
        self.action(WindowAction::Maximize)
    }

    /// Restores this window from its minimized or maximized state.
    ///
    /// # Errors
    ///
    /// Returns an error when this window or application is no longer live, or when the bounded
    /// command queue is full.
    pub fn restore(&self) -> Result<(), WindowError> {
        self.action(WindowAction::Restore)
    }

    /// Requests an ordinary native close; dropping this handle does not close the window.
    ///
    /// # Errors
    ///
    /// Returns an error when this window or application is no longer live, or when the bounded
    /// command queue is full.
    pub fn close(&self) -> Result<(), WindowError> {
        self.action(WindowAction::Close)
    }

    #[doc(hidden)]
    pub fn replace_info(&self, info: WindowInfo) {
        debug_assert_eq!(self.id, info.id);
        self.info.set(info);
    }

    fn action(&self, action: WindowAction) -> Result<(), WindowError> {
        self.dispatcher()?.window_action(self.id, action)
    }

    fn dispatcher(&self) -> Result<Rc<dyn WindowDispatcher>, WindowError> {
        self.dispatcher.upgrade().ok_or(WindowError::AppStopped)
    }
}

struct DisconnectedWindowDispatcher;

impl WindowDispatcher for DisconnectedWindowDispatcher {
    fn open_window(&self, _window: Window, _root: WindowRoot) -> Result<WindowHandle, WindowError> {
        Err(WindowError::AppStopped)
    }

    fn update_window(&self, id: WindowId, _update: WindowUpdate) -> Result<(), WindowError> {
        Err(WindowError::Closed(id))
    }

    fn window_action(&self, id: WindowId, _action: WindowAction) -> Result<(), WindowError> {
        Err(WindowError::Closed(id))
    }

    fn windows(&self) -> Vec<WindowHandle> {
        Vec::new()
    }

    fn active_window(&self) -> Option<WindowHandle> {
        None
    }
}

/// Inputs needed to mount one erased persistent root into a native window driver.
#[doc(hidden)]
pub struct WindowMountContext {
    pub app_state: AppStateStore,
    pub window_state: WindowStateStore,
    pub app_events: AppEvents,
    pub app_handle: AppHandle,
    pub window_handle: WindowHandle,
    pub owners: OwnerRegistry,
    pub spawner: UiSpawner,
}

/// A mounted component host erased only at the platform boundary.
#[doc(hidden)]
pub trait ErasedComponentHost {
    fn render(&mut self) -> Result<&ElementNode, RenderError>;
    fn render_dirty(&mut self, dirty: &[OwnerId]) -> Result<&ElementNode, RenderError>;
    fn contains_owner(&self, owner: OwnerId) -> bool;
    fn did_paint(&mut self);
    fn unmount(&mut self);
    fn element_snapshot(&self) -> Option<Rc<ElementNode>>;
    fn reactive_stats(&self) -> ReactiveStats;
}

struct TypedComponentHost<C: Render>(ComponentHost<C>);

impl<C: Render> ErasedComponentHost for TypedComponentHost<C> {
    fn render(&mut self) -> Result<&ElementNode, RenderError> {
        self.0.render()
    }

    fn render_dirty(&mut self, dirty: &[OwnerId]) -> Result<&ElementNode, RenderError> {
        self.0.render_dirty(dirty)
    }

    fn contains_owner(&self, owner: OwnerId) -> bool {
        self.0.contains_owner(owner)
    }

    fn did_paint(&mut self) {
        self.0.did_paint();
    }

    fn unmount(&mut self) {
        self.0.unmount();
    }

    fn element_snapshot(&self) -> Option<Rc<ElementNode>> {
        self.0.element_snapshot()
    }

    fn reactive_stats(&self) -> ReactiveStats {
        self.0.reactive_stats()
    }
}

/// Result of mounting a type-erased window root.
#[doc(hidden)]
pub struct MountedWindowRoot {
    pub owner: OwnerId,
    pub host: Box<dyn ErasedComponentHost>,
}

trait MountWindowRoot {
    fn mount(self: Box<Self>, context: WindowMountContext) -> MountedWindowRoot;
}

struct TypedWindowRoot<C: Render> {
    root: C,
    event_registrar: Option<fn(&C, &mut Context<C>, &mut EventBindings)>,
}

impl<C: Render> MountWindowRoot for TypedWindowRoot<C> {
    fn mount(self: Box<Self>, context: WindowMountContext) -> MountedWindowRoot {
        let WindowMountContext {
            app_state,
            window_state,
            app_events,
            app_handle,
            window_handle,
            owners,
            spawner,
        } = context;
        let component_context = Context::with_window_services(
            app_state,
            window_state,
            app_events,
            app_handle,
            window_handle,
            owners,
            &spawner,
        );
        let owner = component_context.owner_id();
        let component = Rc::new(self.root);
        let host = match self.event_registrar {
            Some(registrar) => {
                ComponentHost::new_with_event_registrar(component, component_context, registrar)
            }
            None => ComponentHost::new(component, component_context),
        };
        MountedWindowRoot {
            owner,
            host: Box::new(TypedComponentHost(host)),
        }
    }
}

/// Type-erased persistent root carried through the portable open-window command.
#[doc(hidden)]
pub struct WindowRoot {
    root_type: &'static str,
    inner: Box<dyn MountWindowRoot>,
}

impl WindowRoot {
    #[must_use]
    pub fn new<C: Render>(root: C) -> Self {
        Self {
            root_type: std::any::type_name::<C>(),
            inner: Box::new(TypedWindowRoot {
                root,
                event_registrar: None,
            }),
        }
    }

    #[must_use]
    pub fn new_eventful<C: Render + Eventful>(root: C) -> Self {
        Self {
            root_type: std::any::type_name::<C>(),
            inner: Box::new(TypedWindowRoot {
                root,
                event_registrar: Some(register_eventful::<C>),
            }),
        }
    }

    #[must_use]
    pub const fn root_type(&self) -> &'static str {
        self.root_type
    }

    #[must_use]
    pub fn mount(self, context: WindowMountContext) -> MountedWindowRoot {
        self.inner.mount(context)
    }
}

fn register_eventful<C: Render + Eventful>(
    root: &C,
    cx: &mut Context<C>,
    bindings: &mut EventBindings,
) {
    root.bind_events(cx, bindings);
}
