#![forbid(unsafe_code)]

//! Tokio-backed application runtime and a main-thread local UI executor.
//!
//! Tokio owns timers and I/O readiness on worker threads. [`UiExecutor`] uses
//! `async-task` to poll `!Send` futures only when the `AppKit` event loop calls
//! [`UiExecutor::run_ready`] on the thread that created the runtime.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak as RcWeak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, ThreadId};

use anmixiu_reactive::{OwnerId, OwnerRegistry};
use async_task::Runnable;
use tokio::runtime::{Builder, Handle, Runtime};

/// Hard upper bound for owner-bound UI tasks in one application.
pub const MAX_UI_TASKS: usize = 4_096;

/// Maximum local future polls performed in one `AppKit` wake turn.
pub const MAX_UI_POLLS_PER_TURN: usize = 4_096;

/// Fixed Tokio worker count for timer and I/O readiness work in one UI application.
pub const APP_WORKER_THREADS: usize = 2;

/// Error returned when Tokio cannot create its worker runtime.
#[derive(Debug)]
pub struct RuntimeBuildError(std::io::Error);

impl fmt::Display for RuntimeBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to build Anmixiu Tokio runtime: {}",
            self.0
        )
    }
}

impl Error for RuntimeBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// A per-application Tokio runtime plus its UI-thread executor.
pub struct AppRuntime {
    runtime: Runtime,
    ui: UiExecutor,
    worker_thread_count: usize,
}

impl fmt::Debug for AppRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppRuntime")
            .field("ui", &self.ui)
            .finish_non_exhaustive()
    }
}

impl AppRuntime {
    /// Creates one multithread Tokio runtime and a UI executor.
    ///
    /// `wake_appkit` must only request an event-loop wake; it may be invoked by
    /// a Tokio worker thread. Ready futures are still polled by `run_ready`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeBuildError`] if Tokio cannot initialize its workers or
    /// platform I/O driver.
    pub fn new(wake_appkit: impl Fn() + Send + Sync + 'static) -> Result<Self, RuntimeBuildError> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(APP_WORKER_THREADS)
            .enable_all()
            .thread_name("anmixiu-async")
            .build()
            .map_err(RuntimeBuildError)?;
        let ui = UiExecutor::new(runtime.handle().clone(), wake_appkit);
        Ok(Self {
            runtime,
            ui,
            worker_thread_count: APP_WORKER_THREADS,
        })
    }

    /// Returns the main-thread local executor.
    #[must_use]
    pub fn ui(&self) -> &UiExecutor {
        &self.ui
    }

    /// Returns Tokio's Send-capable timer/I/O handle.
    #[must_use]
    pub fn tokio_handle(&self) -> &Handle {
        self.runtime.handle()
    }

    /// Returns the configured number of background Tokio workers.
    #[must_use]
    pub const fn worker_thread_count(&self) -> usize {
        self.worker_thread_count
    }
}

struct ReadyQueue {
    // Key: a scheduled task has at most one Runnable. Invalidation: popping it
    // to poll. Capacity: bounded by MAX_UI_TASKS because every runnable belongs
    // to one active task and async-task never schedules two for the same task.
    runnables: Mutex<VecDeque<Runnable>>,
    wake_requested: AtomicBool,
    wake_appkit: Arc<dyn Fn() + Send + Sync>,
}

impl ReadyQueue {
    fn new(wake_appkit: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            runnables: Mutex::new(VecDeque::with_capacity(MAX_UI_TASKS)),
            wake_requested: AtomicBool::new(false),
            wake_appkit: Arc::new(wake_appkit),
        }
    }

    fn schedule(&self, runnable: Runnable) {
        let mut runnables = self
            .runnables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        debug_assert!(
            runnables.len() < MAX_UI_TASKS,
            "one-runnable-per-task invariant keeps the ready queue bounded"
        );
        runnables.push_back(runnable);
        drop(runnables);
        self.request_wake();
    }

    fn request_wake(&self) {
        if !self.wake_requested.swap(true, Ordering::AcqRel) {
            (self.wake_appkit)();
        }
    }

    fn begin_turn(&self) {
        self.wake_requested.store(false, Ordering::Release);
    }

    fn pop(&self) -> Option<Runnable> {
        self.runnables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    fn len(&self) -> usize {
        self.runnables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn finish_turn(&self) -> bool {
        let runnables = self
            .runnables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let has_ready_tasks = !runnables.is_empty();
        if has_ready_tasks {
            drop(runnables);
            self.request_wake();
        } else {
            // Keep the queue locked while rearming. A worker that has not yet
            // enqueued will observe `false` after acquiring the lock and issue
            // the next AppKit wake; an already-enqueued runnable is handled by
            // the branch above.
            self.wake_requested.store(false, Ordering::Release);
        }
        has_ready_tasks
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct UiTaskId(u64);

struct TaskControl {
    cancelled: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl TaskControl {
    fn new() -> Self {
        Self {
            cancelled: Cell::new(false),
            waker: RefCell::new(None),
        }
    }

    fn cancel(&self) {
        self.cancelled.set(true);
        if let Some(waker) = self.waker.borrow().as_ref() {
            waker.wake_by_ref();
        }
    }
}

#[derive(Default)]
struct OwnerTasks {
    tasks: HashMap<UiTaskId, RcWeak<TaskControl>>,
}

#[derive(Default)]
struct TaskRegistry {
    owners: HashMap<OwnerId, OwnerTasks>,
    next_task_id: u64,
}

impl TaskRegistry {
    fn active_task_count(&self) -> usize {
        self.owners.values().map(|owner| owner.tasks.len()).sum()
    }
}

struct UiState {
    active: Cell<bool>,
    tasks: RefCell<TaskRegistry>,
}

impl UiState {
    fn complete(&self, owner: OwnerId, task: UiTaskId) {
        if let Some(owner_tasks) = self.tasks.borrow_mut().owners.get_mut(&owner) {
            owner_tasks.tasks.remove(&task);
        }
    }

    fn cancel_owner(&self, owner: OwnerId) {
        let Some(owner_tasks) = self.tasks.borrow_mut().owners.remove(&owner) else {
            return;
        };
        for control in owner_tasks
            .tasks
            .into_values()
            .filter_map(|task| task.upgrade())
        {
            control.cancel();
        }
    }

    fn cancel_all(&self) {
        let owners: Vec<_> = self.tasks.borrow().owners.keys().copied().collect();
        for owner in owners {
            self.cancel_owner(owner);
        }
    }
}

struct CompletionGuard {
    state: RcWeak<UiState>,
    owner: OwnerId,
    task: UiTaskId,
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state.complete(self.owner, self.task);
        }
    }
}

struct OwnerBoundFuture<F> {
    future: Pin<Box<F>>,
    control: Rc<TaskControl>,
    _completion: CompletionGuard,
}

impl<F: Future<Output = ()>> Future for OwnerBoundFuture<F> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.control.cancelled.get() {
            this.control.waker.borrow_mut().take();
            return Poll::Ready(());
        }
        this.control.waker.borrow_mut().replace(cx.waker().clone());
        let result = this.future.as_mut().poll(cx);
        if result.is_ready() {
            this.control.waker.borrow_mut().take();
        }
        result
    }
}

/// Counts used to verify bounded UI-task bookkeeping.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UiTaskStats {
    /// Number of unfinished local tasks.
    pub active_task_count: usize,
    /// Number of live owners that have ever spawned a task.
    pub bound_owner_count: usize,
    /// Number of local tasks ready to poll.
    pub ready_task_count: usize,
}

/// Result of one bounded UI event-loop drain.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UiRunReport {
    /// Number of futures polled in this turn.
    pub poll_count: usize,
    /// Whether the poll guard left ready work for another `AppKit` turn.
    pub has_ready_tasks: bool,
}

/// Reason a local task could not be spawned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    /// Spawn was attempted away from the UI thread.
    WrongThread,
    /// The component/window owner has already unmounted.
    OwnerNotAlive,
    /// The per-application hard task capacity was reached.
    CapacityReached,
    /// The application UI executor has already shut down.
    ExecutorStopped,
}

impl fmt::Display for SpawnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongThread => {
                formatter.write_str("UI tasks can only be spawned on the UI thread")
            }
            Self::OwnerNotAlive => formatter.write_str("UI task owner is no longer alive"),
            Self::CapacityReached => formatter.write_str("UI task capacity has been reached"),
            Self::ExecutorStopped => formatter.write_str("UI executor is no longer running"),
        }
    }
}

impl Error for SpawnError {}

/// Reason ready UI tasks could not be run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WrongUiThread;

impl fmt::Display for WrongUiThread {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UI tasks can only be polled on the UI thread")
    }
}

impl Error for WrongUiThread {}

/// Cloneable, main-thread handle injected into component [`Context`](https://docs.rs/anmixiu-core).
///
/// It retains the reactive registry needed to reject dead owners and install
/// automatic cancellation cleanup. Like [`UiExecutor`], it is intentionally
/// `!Send`/`!Sync`.
#[derive(Clone)]
pub struct UiSpawner {
    state: Rc<UiState>,
    ready: Arc<ReadyQueue>,
    owners: OwnerRegistry,
    ui_thread: ThreadId,
}

impl fmt::Debug for UiSpawner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiSpawner")
            .field("ui_thread", &self.ui_thread)
            .finish_non_exhaustive()
    }
}

impl UiSpawner {
    /// Spawns a `!Send` future bound to a component or window fallback owner.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError`] when called off the UI thread, after owner or
    /// executor teardown, or after reaching [`MAX_UI_TASKS`].
    pub fn spawn<F>(&self, owner: OwnerId, future: F) -> Result<(), SpawnError>
    where
        F: Future<Output = ()> + 'static,
    {
        spawn_local_task(
            &self.state,
            &self.ready,
            self.ui_thread,
            &self.owners,
            owner,
            future,
        )
    }

    /// Cancels all unfinished tasks owned by `owner`.
    pub fn cancel_owner(&self, owner: OwnerId) {
        self.state.cancel_owner(owner);
    }
}

/// Main-thread executor for owner-bound `!Send` futures.
///
/// The `Rc` state intentionally makes this type `!Send` and `!Sync`.
pub struct UiExecutor {
    state: Rc<UiState>,
    ready: Arc<ReadyQueue>,
    tokio: Handle,
    ui_thread: ThreadId,
}

impl fmt::Debug for UiExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiExecutor")
            .field("ui_thread", &self.ui_thread)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl UiExecutor {
    fn new(handle: Handle, wake_appkit: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            state: Rc::new(UiState {
                active: Cell::new(true),
                tasks: RefCell::new(TaskRegistry::default()),
            }),
            ready: Arc::new(ReadyQueue::new(wake_appkit)),
            tokio: handle,
            ui_thread: thread::current().id(),
        }
    }

    /// Spawns a `!Send` UI future and binds cancellation to `owner` teardown.
    ///
    /// No task handle needs to be retained by the caller. Bookkeeping is
    /// removed automatically on completion; owner removal cancels pending work.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError`] when called off the UI thread, after owner or
    /// executor teardown, or after reaching [`MAX_UI_TASKS`].
    pub fn spawn<F>(
        &self,
        owners: &OwnerRegistry,
        owner: OwnerId,
        future: F,
    ) -> Result<(), SpawnError>
    where
        F: Future<Output = ()> + 'static,
    {
        spawn_local_task(
            &self.state,
            &self.ready,
            self.ui_thread,
            owners,
            owner,
            future,
        )
    }

    /// Creates a cloneable owner-aware spawn handle for component contexts.
    #[must_use]
    pub fn spawner(&self, owners: OwnerRegistry) -> UiSpawner {
        UiSpawner {
            state: self.state.clone(),
            ready: self.ready.clone(),
            owners,
            ui_thread: self.ui_thread,
        }
    }

    /// Polls ready UI futures on the creating thread, with a starvation guard.
    ///
    /// # Errors
    ///
    /// Returns [`WrongUiThread`] when called from any other thread.
    pub fn run_ready(&self) -> Result<UiRunReport, WrongUiThread> {
        if thread::current().id() != self.ui_thread {
            return Err(WrongUiThread);
        }
        self.ready.begin_turn();
        let _tokio_context = self.tokio.enter();
        let mut poll_count = 0;
        while poll_count < MAX_UI_POLLS_PER_TURN {
            let Some(runnable) = self.ready.pop() else {
                break;
            };
            runnable.run();
            poll_count += 1;
        }
        let has_ready_tasks = self.ready.finish_turn();
        Ok(UiRunReport {
            poll_count,
            has_ready_tasks,
        })
    }

    /// Cancels every task associated with `owner`.
    ///
    /// Normally [`OwnerRegistry::remove_owner`] calls this automatically via
    /// the cleanup registered by [`spawn`](Self::spawn).
    pub fn cancel_owner(&self, owner: OwnerId) {
        self.state.cancel_owner(owner);
    }

    /// Returns bounded task, owner binding, and runnable queue counts.
    #[must_use]
    pub fn stats(&self) -> UiTaskStats {
        let tasks = self.state.tasks.borrow();
        UiTaskStats {
            active_task_count: tasks.active_task_count(),
            bound_owner_count: tasks.owners.len(),
            ready_task_count: self.ready.len(),
        }
    }
}

impl Drop for UiExecutor {
    fn drop(&mut self) {
        self.state.active.set(false);
        if thread::current().id() == self.ui_thread {
            self.state.cancel_all();
            let _ = self.run_ready();
        }
    }
}

fn spawn_local_task<F>(
    state: &Rc<UiState>,
    ready: &Arc<ReadyQueue>,
    ui_thread: ThreadId,
    owners: &OwnerRegistry,
    owner: OwnerId,
    future: F,
) -> Result<(), SpawnError>
where
    F: Future<Output = ()> + 'static,
{
    if !state.active.get() {
        return Err(SpawnError::ExecutorStopped);
    }
    if thread::current().id() != ui_thread {
        return Err(SpawnError::WrongThread);
    }
    if !owners.is_alive(owner) {
        return Err(SpawnError::OwnerNotAlive);
    }

    let needs_binding = !state.tasks.borrow().owners.contains_key(&owner);
    if needs_binding {
        let weak_state = Rc::downgrade(state);
        if !owners.register_cleanup(owner, move || {
            if let Some(state) = weak_state.upgrade() {
                state.cancel_owner(owner);
            }
        }) {
            return Err(SpawnError::OwnerNotAlive);
        }
        state
            .tasks
            .borrow_mut()
            .owners
            .insert(owner, OwnerTasks::default());
    }

    let mut registry = state.tasks.borrow_mut();
    if registry.active_task_count() >= MAX_UI_TASKS {
        return Err(SpawnError::CapacityReached);
    }
    let task_id = UiTaskId(registry.next_task_id);
    registry.next_task_id = registry
        .next_task_id
        .checked_add(1)
        .expect("UI task id space exhausted");
    let control = Rc::new(TaskControl::new());
    registry
        .owners
        .get_mut(&owner)
        .expect("live owner was bound above")
        .tasks
        .insert(task_id, Rc::downgrade(&control));
    drop(registry);

    let owner_bound = OwnerBoundFuture {
        future: Box::pin(future),
        control,
        _completion: CompletionGuard {
            state: Rc::downgrade(state),
            owner,
            task: task_id,
        },
    };
    let ready_for_wake = ready.clone();
    let (runnable, task) = async_task::spawn_local(owner_bound, move |runnable| {
        ready_for_wake.schedule(runnable);
    });
    task.detach();
    ready.schedule(runnable);
    Ok(())
}
