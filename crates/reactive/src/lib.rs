#![forbid(unsafe_code)]

//! Main-thread reactive primitives for Anmixiu components.
//!
//! A [`Signal`] can be created anywhere, but reading it only subscribes while an
//! explicit [`OwnerRegistry::observe`] scope is active. Mutations enqueue dirty
//! owners; they never render components directly.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::panic::Location;
use std::rc::{Rc, Weak};

thread_local! {
    static NEXT_OWNER_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_SIGNAL_ID: Cell<u64> = const { Cell::new(1) };
    static CURRENT_OBSERVER: RefCell<Option<ObserverContext>> = const { RefCell::new(None) };
}

/// Stable identity for one mounted component or window fallback owner.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OwnerId(u64);

impl OwnerId {
    fn next() -> Self {
        NEXT_OWNER_ID.with(|next| {
            let id = next.get();
            next.set(
                id.checked_add(1)
                    .expect("reactive owner id space exhausted"),
            );
            Self(id)
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SignalId(u64);

impl SignalId {
    fn next() -> Self {
        NEXT_SIGNAL_ID.with(|next| {
            let id = next.get();
            next.set(id.checked_add(1).expect("signal id space exhausted"));
            Self(id)
        })
    }
}

/// A FIFO dirty-owner queue with insertion deduplication.
#[derive(Debug, Default)]
pub struct DirtyQueue {
    queued: HashSet<OwnerId>,
    order: VecDeque<OwnerId>,
}

impl DirtyQueue {
    /// Enqueues `owner`, returning whether this was its first pending entry.
    pub fn mark(&mut self, owner: OwnerId) -> bool {
        if self.queued.insert(owner) {
            self.order.push_back(owner);
            true
        } else {
            false
        }
    }

    /// Removes and returns all dirty owners in first-invalidation order.
    pub fn take(&mut self) -> Vec<OwnerId> {
        self.queued.clear();
        self.order.drain(..).collect()
    }

    /// Removes a pending owner, if present.
    pub fn remove(&mut self, owner: OwnerId) -> bool {
        if !self.queued.remove(&owner) {
            return false;
        }
        self.order.retain(|queued| *queued != owner);
        true
    }

    /// Returns the number of unique pending owners.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queued.len()
    }

    /// Iterates the pending owners in first-invalidation order without draining.
    pub fn iter(&self) -> impl Iterator<Item = OwnerId> + '_ {
        self.order.iter().copied()
    }

    /// Returns whether the queue contains no owners.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }
}

trait SourceSubscription {
    fn remove_owner(&self, owner: OwnerId);
}

struct OwnerRecord {
    dependencies: HashMap<SignalId, Weak<dyn SourceSubscription>>,
    cleanup: Vec<Box<dyn FnOnce()>>,
}

impl OwnerRecord {
    fn new() -> Self {
        Self {
            dependencies: HashMap::new(),
            cleanup: Vec::new(),
        }
    }
}

struct RegistryInner {
    owners: RefCell<HashMap<OwnerId, OwnerRecord>>,
    dirty: RefCell<DirtyQueue>,
    // Value is the source location of the most recent `request_animation_frame` for that owner,
    // so a runaway-animation diagnostic can point at the exact call site in user code.
    animating: RefCell<HashMap<OwnerId, &'static Location<'static>>>,
}

/// Current bounded bookkeeping counts for a reactive owner registry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReactiveStats {
    /// Number of mounted/live owners.
    pub live_owner_count: usize,
    /// Number of `(owner, signal)` dependency edges.
    pub subscription_count: usize,
    /// Number of deduplicated dirty owners waiting for a frame.
    pub dirty_owner_count: usize,
    /// Number of owners with a pending animation-frame request.
    pub animating_owner_count: usize,
}

/// Owns component identities, dependency edges, and the dirty queue.
///
/// This type is intentionally `!Send`: UI signals and their owners stay on the
/// application main thread in the MVP.
#[derive(Clone)]
pub struct OwnerRegistry {
    inner: Rc<RegistryInner>,
}

impl Default for OwnerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for OwnerRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnerRegistry")
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl OwnerRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RegistryInner {
                owners: RefCell::new(HashMap::new()),
                dirty: RefCell::new(DirtyQueue::default()),
                animating: RefCell::new(HashMap::new()),
            }),
        }
    }

    /// Allocates and mounts a fresh owner identity.
    #[must_use]
    pub fn create_owner(&self) -> OwnerId {
        let owner = OwnerId::next();
        let replaced = self
            .inner
            .owners
            .borrow_mut()
            .insert(owner, OwnerRecord::new());
        debug_assert!(replaced.is_none(), "owner ids are globally unique");
        owner
    }

    /// Returns whether `owner` is still mounted in this registry.
    #[must_use]
    pub fn is_alive(&self, owner: OwnerId) -> bool {
        self.inner.owners.borrow().contains_key(&owner)
    }

    /// Runs `render` in an explicit dependency-tracking scope.
    ///
    /// The previous dependency set is detached before the closure runs. `None`
    /// means the owner was already removed, so the closure was not invoked.
    pub fn observe<R>(&self, owner: OwnerId, render: impl FnOnce() -> R) -> Option<R> {
        if !self.is_alive(owner) {
            return None;
        }
        self.clear_dependencies(owner);
        let _guard = ObserverGuard::enter(ObserverContext {
            owner,
            registry: Rc::downgrade(&self.inner),
        });
        Some(render())
    }

    /// Marks a live owner dirty and returns whether it was newly enqueued.
    #[must_use]
    pub fn mark_dirty(&self, owner: OwnerId) -> bool {
        if !self.is_alive(owner) {
            return false;
        }
        self.inner.dirty.borrow_mut().mark(owner)
    }

    /// Takes the current frame's dirty owners.
    #[must_use]
    pub fn take_dirty(&self) -> Vec<OwnerId> {
        self.inner.dirty.borrow_mut().take()
    }

    /// Returns the number of unique owners waiting for a frame.
    #[must_use]
    pub fn dirty_len(&self) -> usize {
        self.inner.dirty.borrow().len()
    }

    /// Removes a single owner from the dirty queue, if present. Returns whether it was queued.
    ///
    /// Used to drop only a stalled owner's pending frame while leaving other owners (e.g. ones
    /// driving a legitimate animation) enqueued.
    #[must_use]
    pub fn clear_dirty(&self, owner: OwnerId) -> bool {
        self.inner.dirty.borrow_mut().remove(owner)
    }

    /// Returns the currently dirty owners without draining the queue.
    ///
    /// Unlike [`take_dirty`](Self::take_dirty) this leaves the queue intact, so a caller can
    /// inspect which owners became dirty *during* a render (e.g. to separate a declared animation
    /// re-request from an anonymous self-invalidation) without cancelling the pending frame.
    #[must_use]
    pub fn dirty_snapshot(&self) -> Vec<OwnerId> {
        self.inner.dirty.borrow().iter().collect()
    }

    /// Requests that `owner` be re-rendered on the next display frame as part of an ongoing
    /// animation, and marks it dirty. Returns whether the owner is live (a dead owner is ignored).
    ///
    /// This is the declared-intent channel that separates a legitimate per-frame animation from an
    /// accidental self-invalidation: frames driven by an owner in the animating set are exempt from
    /// the render-loop guard because they are paced by the display link (vsync), whereas an owner
    /// that keeps dirtying itself *without* requesting animation is treated as a runaway loop. The
    /// request is consumed each frame ([`take_animating`](Self::take_animating)); a component
    /// continues an animation by calling this again from its next render, and stops simply by not
    /// calling it (browser `requestAnimationFrame` semantics).
    ///
    /// `#[track_caller]` records the source location of the call so a runaway-animation warning can
    /// name the exact line in user code (see [`take_animating_with_sites`](Self::take_animating_with_sites)).
    #[track_caller]
    #[must_use]
    pub fn request_animation_frame(&self, owner: OwnerId) -> bool {
        self.request_animation_frame_at(owner, Location::caller())
    }

    /// [`request_animation_frame`](Self::request_animation_frame) with an explicit call site.
    ///
    /// Wrappers (e.g. `Context::request_animation_frame`) call this with `Location::caller()` so the
    /// recorded site is the application code that asked for the frame, not the wrapper itself.
    #[must_use]
    pub fn request_animation_frame_at(
        &self,
        owner: OwnerId,
        site: &'static Location<'static>,
    ) -> bool {
        if !self.is_alive(owner) {
            return false;
        }
        self.inner.animating.borrow_mut().insert(owner, site);
        let _ = self.inner.dirty.borrow_mut().mark(owner);
        true
    }

    /// Takes and clears the owners that requested an animation frame this turn.
    #[must_use]
    pub fn take_animating(&self) -> Vec<OwnerId> {
        self.inner.animating.borrow_mut().drain().map(|(owner, _)| owner).collect()
    }

    /// Takes and clears the animating owners together with the source location of each one's most
    /// recent `request_animation_frame` call. Used by diagnostics to point at the offending line.
    #[must_use]
    pub fn take_animating_with_sites(&self) -> Vec<(OwnerId, &'static Location<'static>)> {
        self.inner.animating.borrow_mut().drain().collect()
    }

    /// Returns the number of owners with a pending animation-frame request.
    #[must_use]
    pub fn animating_len(&self) -> usize {
        self.inner.animating.borrow().len()
    }

    /// Registers synchronous teardown work for a live owner.
    ///
    /// Callbacks run exactly once during [`remove_owner`](Self::remove_owner).
    pub fn register_cleanup(&self, owner: OwnerId, cleanup: impl FnOnce() + 'static) -> bool {
        let mut owners = self.inner.owners.borrow_mut();
        let Some(record) = owners.get_mut(&owner) else {
            return false;
        };
        record.cleanup.push(Box::new(cleanup));
        true
    }

    /// Unmounts an owner, dropping dependency edges before running cleanup.
    #[must_use]
    pub fn remove_owner(&self, owner: OwnerId) -> bool {
        let Some(record) = self.inner.owners.borrow_mut().remove(&owner) else {
            return false;
        };
        self.inner.dirty.borrow_mut().remove(owner);
        // Unmounting silences any ongoing animation the owner had requested (cf. Flutter muting a
        // Ticker when its element leaves the tree), so a dead owner cannot keep driving frames.
        self.inner.animating.borrow_mut().remove(&owner);
        for source in record.dependencies.into_values() {
            if let Some(source) = source.upgrade() {
                source.remove_owner(owner);
            }
        }
        for cleanup in record.cleanup {
            cleanup();
        }
        true
    }

    /// Returns bounded owner, dependency, and dirty queue counts.
    #[must_use]
    pub fn stats(&self) -> ReactiveStats {
        let owners = self.inner.owners.borrow();
        ReactiveStats {
            live_owner_count: owners.len(),
            subscription_count: owners.values().map(|owner| owner.dependencies.len()).sum(),
            dirty_owner_count: self.inner.dirty.borrow().len(),
            animating_owner_count: self.inner.animating.borrow().len(),
        }
    }

    fn clear_dependencies(&self, owner: OwnerId) {
        let dependencies = {
            let mut owners = self.inner.owners.borrow_mut();
            let Some(record) = owners.get_mut(&owner) else {
                return;
            };
            std::mem::take(&mut record.dependencies)
        };
        for source in dependencies.into_values() {
            if let Some(source) = source.upgrade() {
                source.remove_owner(owner);
            }
        }
    }
}

#[derive(Clone)]
struct ObserverContext {
    owner: OwnerId,
    registry: Weak<RegistryInner>,
}

struct ObserverGuard {
    previous: Option<ObserverContext>,
}

impl ObserverGuard {
    fn enter(observer: ObserverContext) -> Self {
        let previous = CURRENT_OBSERVER.with(|current| current.replace(Some(observer)));
        Self { previous }
    }
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        CURRENT_OBSERVER.with(|current| {
            current.replace(self.previous.take());
        });
    }
}

struct SignalInner<T> {
    id: SignalId,
    value: RefCell<T>,
    subscribers: RefCell<HashMap<OwnerId, Weak<RegistryInner>>>,
}

impl<T> SourceSubscription for SignalInner<T> {
    fn remove_owner(&self, owner: OwnerId) {
        self.subscribers.borrow_mut().remove(&owner);
    }
}

/// A shared, main-thread reactive value.
///
/// Cloning a signal only clones an `Rc` handle. A signal is intentionally
/// `!Send`/`!Sync` in the MVP, avoiding locks in render and input hot paths.
pub struct Signal<T> {
    inner: Rc<SignalInner<T>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Default + 'static> Default for Signal<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: fmt::Debug + 'static> fmt::Debug for Signal<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Signal")
            .field("value", &self.inner.value.borrow())
            .field("subscriber_count", &self.subscriber_count())
            .finish_non_exhaustive()
    }
}

impl<T: 'static> Signal<T> {
    /// Creates a signal without requiring an application runtime.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            inner: Rc::new(SignalInner {
                id: SignalId::next(),
                value: RefCell::new(value),
                subscribers: RefCell::new(HashMap::new()),
            }),
        }
    }

    /// Reads the value through `read`, tracking the active render observer.
    ///
    /// # Panics
    ///
    /// Panics if `read` re-enters this same signal with a write (`set`/`update`): the value is
    /// borrowed for the duration of the closure. Read the value out first, then write.
    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        self.track();
        let value = self.inner.value.borrow();
        read(&value)
    }

    /// Replaces the value and marks subscribed live owners dirty **unconditionally**, even if the
    /// new value equals the old one.
    ///
    /// Prefer [`set`](Self::set) (which skips the notification when the value is unchanged); reach
    /// for `set_always` only when `T` is not `PartialEq`, or when you deliberately want to notify
    /// on an equal value (e.g. a signal used as an event/"ping").
    pub fn set_always(&self, value: T) {
        // Swap under the borrow, then drop the previous value only after the borrow is released,
        // so a `Drop` impl that reads this signal cannot hit an "already borrowed" panic.
        let previous = {
            let mut slot = self.inner.value.borrow_mut();
            std::mem::replace(&mut *slot, value)
        };
        drop(previous);
        self.notify();
    }

    /// Mutates the value once and marks subscribed live owners dirty afterward.
    ///
    /// This always notifies, since it cannot know whether the closure changed anything. When `T`
    /// is `Clone + PartialEq` and you want to skip no-op mutations, use
    /// [`update_if_changed`](Self::update_if_changed).
    ///
    /// # Panics
    ///
    /// Panics if `update` re-enters this same signal (e.g. `signal.update(|v| *v += signal.get())`):
    /// the value is mutably borrowed for the duration of the closure. Snapshot any needed reads
    /// before calling.
    pub fn update(&self, update: impl FnOnce(&mut T)) {
        {
            let mut value = self.inner.value.borrow_mut();
            update(&mut value);
        }
        self.notify();
    }

    /// Returns the number of currently live owner subscriptions.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        let mut subscribers = self.inner.subscribers.borrow_mut();
        subscribers.retain(|owner, registry| {
            registry
                .upgrade()
                .is_some_and(|registry| registry.owners.borrow().contains_key(owner))
        });
        subscribers.len()
    }

    fn track(&self) {
        let observer = CURRENT_OBSERVER.with(|current| current.borrow().clone());
        let Some(observer) = observer else {
            return;
        };
        let Some(registry) = observer.registry.upgrade() else {
            return;
        };
        if !registry.owners.borrow().contains_key(&observer.owner) {
            return;
        }

        self.inner
            .subscribers
            .borrow_mut()
            .insert(observer.owner, Rc::downgrade(&registry));
        let source: Rc<dyn SourceSubscription> = self.inner.clone();
        let mut owners = registry.owners.borrow_mut();
        if let Some(record) = owners.get_mut(&observer.owner) {
            record
                .dependencies
                .entry(self.inner.id)
                .or_insert_with(|| Rc::downgrade(&source));
        }
    }

    fn notify(&self) {
        self.inner
            .subscribers
            .borrow_mut()
            .retain(|owner, registry| {
                let Some(registry) = registry.upgrade() else {
                    return false;
                };
                if !registry.owners.borrow().contains_key(owner) {
                    return false;
                }
                registry.dirty.borrow_mut().mark(*owner);
                true
            });
    }
}

impl<T: Clone + 'static> Signal<T> {
    /// Clones and returns the current value, tracking the active observer.
    ///
    /// Prefer this over `with` inside a write closure: `get` releases the borrow before returning,
    /// so `signal.set(signal.get() + 1)` is fine while `signal.update(|v| *v += signal.get())` is
    /// not (the latter re-enters the mutable borrow).
    pub fn get(&self) -> T {
        self.with(Clone::clone)
    }
}

impl<T: PartialEq + 'static> Signal<T> {
    /// Replaces the value, marking subscribed owners dirty **only if the value actually changed**.
    ///
    /// This is the default write: setting a signal to the value it already holds (`count.set(1)`
    /// when `count` is `1`) is a no-op and schedules no frame, matching Vue/React/Solid. It also
    /// means a render that writes an unchanged value does not look like a self-invalidation to the
    /// render-loop guard. Use [`set_always`](Self::set_always) to force a notification, or when `T`
    /// is not `PartialEq`.
    ///
    /// # Panics
    ///
    /// Panics if `T`'s `PartialEq` re-enters this signal with a write while the comparison holds
    /// the value borrow. (Comparisons that read the signal are already covered by [`with`].)
    pub fn set(&self, value: T) {
        let previous = {
            let mut slot = self.inner.value.borrow_mut();
            if *slot == value {
                return;
            }
            std::mem::replace(&mut *slot, value)
        };
        // Drop the old value after releasing the borrow (Drop-safe, as in `set_always`).
        drop(previous);
        self.notify();
    }
}

impl<T: Clone + PartialEq + 'static> Signal<T> {
    /// Mutates the value and notifies **only if the mutation changed it**.
    ///
    /// Unlike [`update`](Self::update), this snapshots the previous value (hence `Clone`) and
    /// compares afterward, so a no-op mutation schedules no frame. That snapshot has a cost —
    /// prefer plain `update` for large values you always change, and this for cheap values whose
    /// updates are often no-ops. Returns whether a change (and notification) occurred.
    ///
    /// # Panics
    ///
    /// Same re-entrancy rule as [`update`](Self::update): the value is mutably borrowed for the
    /// duration of the closure.
    pub fn update_if_changed(&self, update: impl FnOnce(&mut T)) -> bool {
        let (changed, previous) = {
            let mut value = self.inner.value.borrow_mut();
            let previous = value.clone();
            update(&mut value);
            (*value != previous, previous)
        };
        drop(previous);
        if changed {
            self.notify();
        }
        changed
    }
}
