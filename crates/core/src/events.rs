use std::{
    any::{Any, TypeId},
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    fmt,
    rc::{Rc, Weak},
};

use anmixiu_reactive::{OwnerCleanup, OwnerId, OwnerRegistry};

use crate::WindowId;

/// Hard upper bound for nested events waiting to be delivered by one app event router.
pub const MAX_PENDING_EVENTS: usize = 4_096;

/// Hard upper bound for events delivered by one synchronous dispatch turn.
pub const MAX_EVENTS_PER_DISPATCH: usize = 4_096;

/// Selects the routing scope for an event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventScope {
    /// Delivers to subscriptions owned by the same persistent Element.
    Owner,
    /// Delivers to subscriptions in the current Window.
    Window,
    /// Delivers to subscriptions in the whole App, including every Window.
    App,
}

/// Ordering hint for one event subscription.
///
/// Higher values run first. Subscriptions with equal priority retain registration order, making
/// dispatch deterministic without requiring callers to invent unique priorities.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EventPriority(i32);

impl EventPriority {
    /// The default priority for ordinary event handlers.
    pub const NORMAL: Self = Self(0);
    /// A convenient priority for handlers that should run before ordinary handlers.
    pub const HIGH: Self = Self(100);
    /// A convenient priority for handlers that should run after ordinary handlers.
    pub const LOW: Self = Self(-100);

    /// Creates an application-defined priority.
    #[must_use]
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// Returns the numeric priority.
    #[must_use]
    pub const fn value(self) -> i32 {
        self.0
    }
}

impl From<i32> for EventPriority {
    fn from(value: i32) -> Self {
        Self::new(value)
    }
}

/// Error returned when event dispatch cannot accept or synchronously deliver more work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventError {
    /// Nested event delivery reached the bounded queue capacity.
    QueueFull { capacity: usize },
    /// One synchronous event turn exhausted its delivery budget.
    DispatchLimitExceeded { limit: usize },
}

impl fmt::Display for EventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull { capacity } => {
                write!(formatter, "event queue reached its capacity of {capacity}")
            }
            Self::DispatchLimitExceeded { limit } => {
                write!(
                    formatter,
                    "event dispatch reached its delivery limit of {limit}"
                )
            }
        }
    }
}

impl std::error::Error for EventError {}

/// Read-only metadata for one currently active event subscription.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventSubscriptionInfo {
    /// Window in which the subscriber was registered.
    pub window_id: WindowId,
    /// Routing scope selected by the subscriber.
    pub scope: EventScope,
    /// Higher priorities are dispatched first.
    pub priority: EventPriority,
    /// Rust type name of the subscribed payload.
    pub event_type: &'static str,
    /// Owner of the subscribing persistent Element.
    pub owner: OwnerId,
}

/// App-owned typed event router.
///
/// The router is intentionally main-thread-only like [`anmixiu_reactive::Signal`]. Event payloads
/// are ordinary Rust values and routing uses [`TypeId`], never string topics. Cloning this handle
/// only clones an `Rc`; the App owns the single underlying subscription registry.
#[derive(Clone)]
pub struct AppEvents {
    inner: Rc<EventRouterInner>,
}

impl Default for AppEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AppEvents {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppEvents")
            .field("subscription_count", &self.subscription_count())
            .finish_non_exhaustive()
    }
}

impl AppEvents {
    /// Creates an empty App event router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Rc::new(EventRouterInner {
                subscriptions: RefCell::new(HashMap::new()),
                order: RefCell::new(Vec::new()),
                next_subscription: Cell::new(1),
                pending: RefCell::new(VecDeque::new()),
                dispatching: Cell::new(false),
            }),
        }
    }

    /// Returns all currently registered subscriptions for `window_id`.
    ///
    /// The snapshot contains event types and routing metadata, not payload values. It is intended
    /// for diagnostics and `DevTools`, not for render or input hot paths.
    #[must_use]
    pub fn subscriptions(&self, window_id: WindowId) -> Vec<EventSubscriptionInfo> {
        let inner = self.inner.subscriptions.borrow();
        let order = self.inner.order.borrow();
        order
            .iter()
            .filter_map(|id| inner.get(id))
            .filter(|subscription| subscription.window_id == window_id)
            .map(Subscriber::info)
            .collect()
    }

    /// Returns the number of active subscriptions in this App.
    #[must_use]
    pub fn subscription_count(&self) -> usize {
        self.inner.subscriptions.borrow().len()
    }

    fn subscribe<E, F>(
        &self,
        owner: OwnerId,
        window_id: WindowId,
        scope: EventScope,
        priority: EventPriority,
        mut handler: F,
    ) -> Subscription
    where
        E: 'static,
        F: FnMut(&E) + 'static,
    {
        let id = self.inner.next_subscription.get();
        self.inner.next_subscription.set(
            id.checked_add(1)
                .expect("event subscription id space exhausted"),
        );

        let callback = Box::new(move |payload: &dyn Any| {
            if let Some(event) = payload.downcast_ref::<E>() {
                handler(event);
            }
        });
        self.inner.subscriptions.borrow_mut().insert(
            id,
            Subscriber {
                owner,
                window_id,
                scope,
                priority,
                event_type: std::any::type_name::<E>(),
                type_id: TypeId::of::<E>(),
                callback: Some(callback),
            },
        );
        self.inner.order.borrow_mut().push(id);

        Subscription {
            state: Rc::new(SubscriptionState {
                router: Rc::downgrade(&self.inner),
                id: Cell::new(Some(id)),
                cleanup: RefCell::new(None),
            }),
        }
    }

    fn emit<E: 'static>(
        &self,
        owner: OwnerId,
        window_id: WindowId,
        payload: E,
        scope: EventScope,
    ) -> Result<(), EventError> {
        self.enqueue(QueuedEvent {
            owner,
            window_id,
            scope,
            type_id: TypeId::of::<E>(),
            payload: Box::new(payload),
        })
    }

    fn enqueue(&self, event: QueuedEvent) -> Result<(), EventError> {
        {
            let mut pending = self.inner.pending.borrow_mut();
            if pending.len() >= MAX_PENDING_EVENTS {
                return Err(EventError::QueueFull {
                    capacity: MAX_PENDING_EVENTS,
                });
            }
            pending.push_back(event);
        }

        if self.inner.dispatching.replace(true) {
            return Ok(());
        }
        let mut dispatch_guard = DispatchGuard::new(self.inner.clone());
        let mut delivered = 0;
        loop {
            if delivered == MAX_EVENTS_PER_DISPATCH {
                let mut pending = self.inner.pending.borrow_mut();
                if !pending.is_empty() {
                    pending.clear();
                    dispatch_guard.complete();
                    return Err(EventError::DispatchLimitExceeded {
                        limit: MAX_EVENTS_PER_DISPATCH,
                    });
                }
            }
            let next = self.inner.pending.borrow_mut().pop_front();
            let Some(event) = next else {
                break;
            };
            delivered += 1;
            self.dispatch(&event);
        }
        dispatch_guard.complete();
        Ok(())
    }

    fn dispatch(&self, event: &QueuedEvent) {
        let subscribers = {
            let subscriptions = self.inner.subscriptions.borrow();
            let order = self.inner.order.borrow();
            let mut matching = order
                .iter()
                .copied()
                .filter(|id| {
                    subscriptions
                        .get(id)
                        .is_some_and(|subscriber| subscriber.matches(event))
                })
                .collect::<Vec<_>>();
            matching.sort_by(|left, right| {
                let left_priority = subscriptions
                    .get(left)
                    .map_or(EventPriority::NORMAL, |subscriber| subscriber.priority);
                let right_priority = subscriptions
                    .get(right)
                    .map_or(EventPriority::NORMAL, |subscriber| subscriber.priority);
                right_priority
                    .cmp(&left_priority)
                    .then_with(|| left.cmp(right))
            });
            matching
        };

        for id in subscribers {
            let callback = {
                let mut subscriptions = self.inner.subscriptions.borrow_mut();
                subscriptions
                    .get_mut(&id)
                    .and_then(|subscriber| subscriber.callback.take())
            };
            let Some(callback) = callback else {
                continue;
            };
            let mut callback = CallbackLease::new(self.inner.clone(), id, callback);
            if let Some(callback) = callback.callback.as_mut() {
                callback(event.payload.as_ref());
            }
        }
    }

    fn remove(&self, id: u64) {
        self.inner.subscriptions.borrow_mut().remove(&id);
        self.inner.order.borrow_mut().retain(|queued| *queued != id);
    }
}

struct EventRouterInner {
    subscriptions: RefCell<HashMap<u64, Subscriber>>,
    order: RefCell<Vec<u64>>,
    next_subscription: Cell<u64>,
    pending: RefCell<VecDeque<QueuedEvent>>,
    dispatching: Cell<bool>,
}

struct Subscriber {
    owner: OwnerId,
    window_id: WindowId,
    scope: EventScope,
    priority: EventPriority,
    event_type: &'static str,
    type_id: TypeId,
    callback: Option<EventCallback>,
}

type EventCallback = Box<dyn FnMut(&dyn Any)>;

impl Subscriber {
    fn matches(&self, event: &QueuedEvent) -> bool {
        self.type_id == event.type_id
            && match event.scope {
                EventScope::Owner => {
                    self.scope == EventScope::Owner
                        && self.window_id == event.window_id
                        && self.owner == event.owner
                }
                EventScope::Window => {
                    self.scope == EventScope::Window && self.window_id == event.window_id
                }
                EventScope::App => self.scope == EventScope::App,
            }
    }

    fn info(&self) -> EventSubscriptionInfo {
        EventSubscriptionInfo {
            window_id: self.window_id,
            scope: self.scope,
            priority: self.priority,
            event_type: self.event_type,
            owner: self.owner,
        }
    }
}

struct QueuedEvent {
    owner: OwnerId,
    window_id: WindowId,
    scope: EventScope,
    type_id: TypeId,
    payload: Box<dyn Any>,
}

struct DispatchGuard {
    inner: Rc<EventRouterInner>,
    completed: bool,
}

impl DispatchGuard {
    fn new(inner: Rc<EventRouterInner>) -> Self {
        Self {
            inner,
            completed: false,
        }
    }

    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.inner.pending.borrow_mut().clear();
        }
        self.inner.dispatching.set(false);
    }
}

struct CallbackLease {
    inner: Rc<EventRouterInner>,
    id: u64,
    callback: Option<EventCallback>,
}

impl CallbackLease {
    fn new(inner: Rc<EventRouterInner>, id: u64, callback: EventCallback) -> Self {
        Self {
            inner,
            id,
            callback: Some(callback),
        }
    }
}

impl Drop for CallbackLease {
    fn drop(&mut self) {
        let Some(callback) = self.callback.take() else {
            return;
        };
        if let Some(subscriber) = self.inner.subscriptions.borrow_mut().get_mut(&self.id) {
            subscriber.callback = Some(callback);
        }
    }
}

struct SubscriptionState {
    router: Weak<EventRouterInner>,
    id: Cell<Option<u64>>,
    cleanup: RefCell<Option<OwnerCleanup>>,
}

impl SubscriptionState {
    fn cancel(&self) {
        let Some(id) = self.id.replace(None) else {
            return;
        };
        if let Some(router) = self.router.upgrade() {
            AppEvents { inner: router }.remove(id);
        }
        self.cleanup.borrow_mut().take();
    }
}

/// RAII handle for one event subscription.
#[must_use = "dropping the subscription immediately cancels it"]
pub struct Subscription {
    state: Rc<SubscriptionState>,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Subscription(..)")
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.state.cancel();
    }
}

/// Owns all subscriptions declared by one mounted Element lifecycle.
#[derive(Debug)]
pub struct EventBindings {
    context: EventContext,
    subscriptions: Vec<Subscription>,
}

impl EventBindings {
    pub(crate) fn new(context: EventContext) -> Self {
        Self {
            context,
            subscriptions: Vec::new(),
        }
    }

    /// Subscribes `handler` and retains its cancellation handle until this binding set is dropped.
    pub fn subscribe<E, F>(
        &mut self,
        scope: EventScope,
        priority: impl Into<EventPriority>,
        handler: F,
    ) where
        E: 'static,
        F: FnMut(&E) + 'static,
    {
        self.subscriptions
            .push(self.context.subscribe(scope, priority, handler));
    }

    /// Returns the number of retained subscriptions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    /// Returns whether no subscriptions are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subscriptions.is_empty()
    }
}

/// A cloneable event handle bound to one Element owner and Window.
#[derive(Clone)]
pub struct EventContext {
    app_events: AppEvents,
    owner: OwnerId,
    window_id: WindowId,
    registry: OwnerRegistry,
}

impl fmt::Debug for EventContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventContext")
            .field("owner", &self.owner)
            .field("window_id", &self.window_id)
            .finish_non_exhaustive()
    }
}

impl EventContext {
    pub(crate) fn new(
        app_events: AppEvents,
        owner: OwnerId,
        window_id: WindowId,
        registry: OwnerRegistry,
    ) -> Self {
        Self {
            app_events,
            owner,
            window_id,
            registry,
        }
    }

    /// Emits an owned payload in the selected scope.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::QueueFull`] when nested dispatch fills the pending queue, or
    /// [`EventError::DispatchLimitExceeded`] when one synchronous turn exhausts its delivery
    /// budget.
    pub fn emit<E: 'static>(&self, payload: E, scope: EventScope) -> Result<(), EventError> {
        self.app_events
            .emit(self.owner, self.window_id, payload, scope)
    }

    /// Subscribes the current owner to an event type.
    ///
    /// Retain the returned handle for as long as delivery is wanted. Dropping it cancels the
    /// subscription immediately; use [`EventBindings::subscribe`] for mount-lifetime retention.
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
        let subscription =
            self.app_events
                .subscribe(self.owner, self.window_id, scope, priority.into(), handler);
        let weak = Rc::downgrade(&subscription.state);
        let cleanup = self.registry.register_cleanup_handle(self.owner, move || {
            if let Some(state) = weak.upgrade() {
                state.cancel();
            }
        });
        if let Some(cleanup) = cleanup {
            subscription.state.cleanup.borrow_mut().replace(cleanup);
        } else {
            subscription.state.cancel();
        }
        subscription
    }

    /// Returns the number of active App subscriptions.
    #[must_use]
    pub fn subscription_count(&self) -> usize {
        self.app_events.subscription_count()
    }

    /// Returns the active subscription metadata for the current Window.
    #[must_use]
    pub fn subscriptions(&self) -> Vec<EventSubscriptionInfo> {
        self.app_events.subscriptions(self.window_id)
    }

    /// Returns the current Window identity.
    #[must_use]
    pub const fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// Returns the current owner identity for diagnostics.
    #[must_use]
    pub const fn owner_id(&self) -> OwnerId {
        self.owner
    }
}
