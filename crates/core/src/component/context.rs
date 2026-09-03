use std::{cell::Cell, future::Future, marker::PhantomData, pin::Pin, rc::Rc};

use anmixiu_reactive::{OwnerId, OwnerRegistry};
use anmixiu_runtime::UiSpawner;

use crate::{
    AppEvents, AppHandle, AppStateStore, EventContext, EventError, EventPriority, EventScope,
    State, Subscription, WindowHandle, WindowId, WindowStateStore, state::required_state,
};

type LocalFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;
type SpawnFn = dyn Fn(LocalFuture);
type OwnerSpawnFn = dyn Fn(OwnerId, LocalFuture);

pub struct Context<C: 'static> {
    app_state: AppStateStore,
    window_state: WindowStateStore,
    app_events: AppEvents,
    window_id: WindowId,
    app_handle: AppHandle,
    window_handle: WindowHandle,
    spawn: Option<Rc<SpawnFn>>,
    pub(super) registry: OwnerRegistry,
    pub(super) owner: OwnerId,
    owner_alive: Rc<Cell<bool>>,
    marker: PhantomData<fn() -> C>,
}

impl<C: 'static> Clone for Context<C> {
    fn clone(&self) -> Self {
        Self {
            app_state: self.app_state.clone(),
            window_state: self.window_state.clone(),
            app_events: self.app_events.clone(),
            window_id: self.window_id,
            app_handle: self.app_handle.clone(),
            window_handle: self.window_handle.clone(),
            spawn: self.spawn.clone(),
            registry: self.registry.clone(),
            owner: self.owner,
            owner_alive: self.owner_alive.clone(),
            marker: PhantomData,
        }
    }
}

impl<C: 'static> Context<C> {
    #[doc(hidden)]
    #[must_use]
    pub fn testing() -> Self {
        Self::testing_with_state(AppStateStore::new(), WindowStateStore::new())
    }

    #[doc(hidden)]
    #[must_use]
    pub fn testing_with_state(app_state: AppStateStore, window_state: WindowStateStore) -> Self {
        Self::testing_with_state_and_events(
            app_state,
            window_state,
            AppEvents::new(),
            WindowId::new(1),
        )
    }

    #[doc(hidden)]
    #[must_use]
    pub fn testing_with_state_and_events(
        app_state: AppStateStore,
        window_state: WindowStateStore,
        app_events: AppEvents,
        window_id: WindowId,
    ) -> Self {
        let registry = OwnerRegistry::new();
        let owner = registry.create_owner();
        Self {
            app_state,
            window_state,
            app_events,
            window_id,
            app_handle: AppHandle::disconnected(),
            window_handle: WindowHandle::disconnected(window_id),
            spawn: None,
            registry,
            owner,
            owner_alive: Rc::new(Cell::new(true)),
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_spawner(
        app_state: AppStateStore,
        window_state: WindowStateStore,
        spawn: impl Fn(LocalFuture) + 'static,
    ) -> Self {
        Self::with_spawner_and_events(
            app_state,
            window_state,
            AppEvents::new(),
            WindowId::new(1),
            spawn,
        )
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_spawner_and_events(
        app_state: AppStateStore,
        window_state: WindowStateStore,
        app_events: AppEvents,
        window_id: WindowId,
        spawn: impl Fn(LocalFuture) + 'static,
    ) -> Self {
        let registry = OwnerRegistry::new();
        let owner = registry.create_owner();
        Self {
            app_state,
            window_state,
            app_events,
            window_id,
            app_handle: AppHandle::disconnected(),
            window_handle: WindowHandle::disconnected(window_id),
            spawn: Some(Rc::new(spawn)),
            registry,
            owner,
            owner_alive: Rc::new(Cell::new(true)),
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_owner_spawner(
        app_state: AppStateStore,
        window_state: WindowStateStore,
        registry: OwnerRegistry,
        spawn: impl Fn(OwnerId, LocalFuture) + 'static,
    ) -> Self {
        Self::with_owner_spawner_and_events(
            app_state,
            window_state,
            AppEvents::new(),
            WindowId::new(1),
            registry,
            spawn,
        )
    }

    #[doc(hidden)]
    #[must_use]
    pub fn with_owner_spawner_and_events(
        app_state: AppStateStore,
        window_state: WindowStateStore,
        app_events: AppEvents,
        window_id: WindowId,
        registry: OwnerRegistry,
        spawn: impl Fn(OwnerId, LocalFuture) + 'static,
    ) -> Self {
        let owner = registry.create_owner();
        let spawn: Rc<OwnerSpawnFn> = Rc::new(spawn);
        let owner_spawn = spawn.clone();
        Self {
            app_state,
            window_state,
            app_events,
            window_id,
            app_handle: AppHandle::disconnected(),
            window_handle: WindowHandle::disconnected(window_id),
            spawn: Some(Rc::new(move |future| owner_spawn(owner, future))),
            registry,
            owner,
            owner_alive: Rc::new(Cell::new(true)),
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn with_window_services(
        app_state: AppStateStore,
        window_state: WindowStateStore,
        app_events: AppEvents,
        app_handle: AppHandle,
        window_handle: WindowHandle,
        registry: OwnerRegistry,
        spawner: &UiSpawner,
    ) -> Self {
        let window_id = window_handle.id();
        let owner = registry.create_owner();
        let owner_spawner = spawner.clone();
        Self {
            app_state,
            window_state,
            app_events,
            window_id,
            app_handle,
            window_handle,
            spawn: Some(Rc::new(move |future| {
                if let Err(error) = owner_spawner.spawn(owner, future) {
                    panic!("Context::spawn failed: {error}");
                }
            })),
            registry,
            owner,
            owner_alive: Rc::new(Cell::new(true)),
            marker: PhantomData,
        }
    }

    #[must_use]
    pub fn state<T: 'static>(&self) -> State<T> {
        required_state(&self.app_state, &self.window_state)
    }

    #[must_use]
    pub fn try_state<T: 'static>(&self) -> Option<State<T>> {
        self.window_state
            .get()
            .or_else(|| self.app_state.get::<T>())
    }

    /// Creates a cloneable event handle bound to this Element owner and Window.
    #[must_use]
    pub fn event_context(&self) -> EventContext {
        EventContext::new(
            self.app_events.clone(),
            self.owner,
            self.window_id,
            self.registry.clone(),
        )
    }

    /// Emits an owned payload in the selected event scope.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::QueueFull`] when nested dispatch fills the pending queue, or
    /// [`EventError::DispatchLimitExceeded`] when one synchronous turn exhausts its delivery
    /// budget.
    pub fn emit<E: 'static>(&self, payload: E, scope: EventScope) -> Result<(), EventError> {
        self.event_context().emit(payload, scope)
    }

    /// Subscribes this Element owner to an event type.
    ///
    /// Retain the returned handle for as long as delivery is wanted. Dropping it cancels the
    /// subscription immediately; [`crate::EventBindings`] retains mount-lifetime subscriptions.
    pub fn subscribe<E, F>(
        &self,
        scope: EventScope,
        priority: impl Into<EventPriority>,
        handler: F,
    ) -> Subscription
    where
        E: 'static,
        F: FnMut(&E) + 'static,
    {
        self.event_context().subscribe(scope, priority, handler)
    }

    /// Returns this Context's Window identity.
    #[must_use]
    pub const fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// Returns a handle for application-level window operations.
    #[must_use]
    pub fn app(&self) -> AppHandle {
        self.app_handle.clone()
    }

    /// Returns the stable native window that owns this component.
    #[must_use]
    pub fn window(&self) -> WindowHandle {
        self.window_handle.clone()
    }

    /// Schedules an owner-bound local UI future.
    ///
    /// # Panics
    ///
    /// Panics when used with a test-only context that has no application runtime.
    pub fn spawn(&mut self, future: impl Future<Output = ()> + 'static) {
        let Some(spawn) = &self.spawn else {
            panic!("Context::spawn requires a running Anmixiu application");
        };
        spawn(Box::pin(future));
    }

    /// Requests that this component be re-rendered on the next display frame, as one step of an
    /// ongoing animation.
    ///
    /// Call this from `render` each frame to keep an animation running (reading a clock or
    /// interpolating some value as you go), and simply stop calling it to end the animation — the
    /// same per-frame model as the browser's `requestAnimationFrame`. Frames driven this way are
    /// paced by the display link and are exempt from the render-loop guard, so continuous
    /// animation is allowed; a component that instead keeps invalidating itself *without*
    /// requesting an animation frame is still treated as a runaway loop.
    ///
    /// Each animated frame currently re-runs the whole component; prefer coarse, low-frequency
    /// animation until compositor-level property animation lands.
    #[track_caller]
    pub fn request_animation_frame(&self) {
        // Forward the caller's location (this line's caller, i.e. the component's render code) so a
        // runaway-animation warning can name the exact call site rather than this wrapper.
        let _ = self
            .registry
            .request_animation_frame_at(self.owner, std::panic::Location::caller());
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn owner_id(&self) -> OwnerId {
        self.owner
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn owner_registry(&self) -> &OwnerRegistry {
        &self.registry
    }

    pub(super) fn deactivate_owner(&self) {
        self.owner_alive.set(false);
    }
}
