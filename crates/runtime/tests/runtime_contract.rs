use std::cell::{Cell, RefCell};
use std::future::{Future, pending};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::Duration;

use anmixiu_reactive::OwnerRegistry;
use anmixiu_runtime::{AppRuntime, MAX_UI_TASKS, SpawnError, UiTaskStats};

#[test]
fn app_runtime_uses_two_worker_threads_for_ui_timer_and_io_work() {
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    assert_eq!(runtime.worker_thread_count(), 2);
}

#[test]
fn ui_future_accepts_non_send_state_and_every_poll_is_on_the_ui_thread() {
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let ui_thread = thread::current().id();
    let poll_threads = Rc::new(RefCell::new(Vec::new()));
    let future = TwoPollFuture {
        poll_threads: poll_threads.clone(),
        first_poll: true,
    };

    runtime
        .ui()
        .spawn(&owners, owner, future)
        .expect("local task spawns");
    let report = runtime.ui().run_ready().expect("runs on UI thread");

    assert_eq!(report.poll_count, 2);
    assert!(!report.has_ready_tasks);
    assert_eq!(&*poll_threads.borrow(), &[ui_thread, ui_thread]);
    assert_eq!(
        runtime.ui().stats(),
        UiTaskStats {
            bound_owner_count: 1,
            ..UiTaskStats::default()
        }
    );
}

#[test]
fn tokio_wakeup_resumes_the_local_future_on_the_ui_thread() {
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let ui_thread = thread::current().id();
    let resumed_on = Rc::new(Cell::new(None));
    let resumed_on_for_task = resumed_on.clone();
    let (value_tx, value_rx) = tokio::sync::oneshot::channel::<()>();

    runtime
        .ui()
        .spawn(&owners, owner, async move {
            value_rx.await.expect("background sender remains alive");
            resumed_on_for_task.set(Some(thread::current().id()));
        })
        .expect("local task spawns");
    runtime.ui().run_ready().expect("initial poll");

    let (sent_tx, sent_rx) = mpsc::channel();
    runtime.tokio_handle().spawn(async move {
        value_tx.send(()).expect("local receiver remains alive");
        sent_tx.send(()).expect("test receiver remains alive");
    });
    sent_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Tokio worker sends without timing sleeps");
    runtime.ui().run_ready().expect("resume poll");

    assert_eq!(resumed_on.get(), Some(ui_thread));
    assert_eq!(
        runtime.ui().stats(),
        UiTaskStats {
            bound_owner_count: 1,
            ..UiTaskStats::default()
        }
    );
}

#[test]
fn removing_reactive_owner_automatically_cancels_and_drops_its_tasks() {
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let dropped = Rc::new(Cell::new(false));
    let guard = DropFlag(dropped.clone());

    runtime
        .ui()
        .spawn(&owners, owner, async move {
            let _guard = guard;
            pending::<()>().await;
        })
        .expect("local task spawns");
    runtime.ui().run_ready().expect("initial poll");
    assert_eq!(runtime.ui().stats().active_task_count, 1);

    assert!(owners.remove_owner(owner));
    runtime.ui().run_ready().expect("cancellation poll");

    assert!(dropped.get());
    assert_eq!(runtime.ui().stats(), UiTaskStats::default());
}

#[test]
fn completion_releases_task_bookkeeping_without_unmount() {
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();

    for _ in 0..1_000 {
        runtime
            .ui()
            .spawn(&owners, owner, async {})
            .expect("local task spawns");
        runtime.ui().run_ready().expect("task completes");
        assert_eq!(runtime.ui().stats().active_task_count, 0);
    }

    assert_eq!(runtime.ui().stats().bound_owner_count, 1);
    assert!(owners.remove_owner(owner));
    assert_eq!(runtime.ui().stats(), UiTaskStats::default());
}

#[test]
fn spawning_for_an_unmounted_owner_is_rejected() {
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    assert!(owners.remove_owner(owner));

    let error = runtime
        .ui()
        .spawn(&owners, owner, async {})
        .expect_err("dead owners cannot gain tasks");
    assert_eq!(error.to_string(), "UI task owner is no longer alive");
}

#[test]
fn reaching_capacity_does_not_bind_a_brand_new_owner() {
    // Regression: the capacity check must run before an owner's cleanup/binding is installed.
    // Otherwise a first spawn that hits the hard cap leaves an empty binding behind and
    // inflates `bound_owner_count` for an owner that owns no tasks.
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let owners = OwnerRegistry::new();
    let saturating_owner = owners.create_owner();
    for _ in 0..MAX_UI_TASKS {
        runtime
            .ui()
            .spawn(&owners, saturating_owner, pending::<()>())
            .expect("tasks spawn up to the hard capacity");
    }
    assert_eq!(runtime.ui().stats().active_task_count, MAX_UI_TASKS);
    assert_eq!(runtime.ui().stats().bound_owner_count, 1);

    let fresh_owner = owners.create_owner();
    let error = runtime
        .ui()
        .spawn(&owners, fresh_owner, async {})
        .expect_err("the hard capacity rejects further spawns");

    assert_eq!(error, SpawnError::CapacityReached);
    assert_eq!(
        runtime.ui().stats().bound_owner_count,
        1,
        "a rejected first spawn must not leave an empty owner binding"
    );
}

#[test]
fn cloneable_spawner_carries_the_owner_registry_for_context_injection() {
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let spawner = runtime.ui().spawner(owners.clone());
    let spawner_clone = spawner.clone();
    let completed = Rc::new(Cell::new(false));
    let completed_for_task = completed.clone();

    spawner_clone
        .spawn(owner, async move { completed_for_task.set(true) })
        .expect("context-style local task spawns");
    runtime.ui().run_ready().expect("task runs");

    assert!(completed.get());
    assert!(owners.remove_owner(owner));
}

#[test]
fn draining_the_queue_rearms_the_next_appkit_wakeup() {
    let wake_count = Arc::new(AtomicUsize::new(0));
    let wake_count_for_hook = wake_count.clone();
    let runtime = AppRuntime::new(move || {
        wake_count_for_hook.fetch_add(1, Ordering::SeqCst);
    })
    .expect("runtime builds");
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();

    runtime
        .ui()
        .spawn(
            &owners,
            owner,
            TwoPollFuture {
                poll_threads: Rc::new(RefCell::new(Vec::new())),
                first_poll: true,
            },
        )
        .expect("first task spawns");
    assert_eq!(wake_count.load(Ordering::SeqCst), 1);
    runtime.ui().run_ready().expect("first task runs");
    let wakes_after_first_turn = wake_count.load(Ordering::SeqCst);

    runtime
        .ui()
        .spawn(&owners, owner, async {})
        .expect("second task spawns");
    assert_eq!(
        wake_count.load(Ordering::SeqCst),
        wakes_after_first_turn + 1
    );
}

#[test]
fn spawner_rejects_work_after_its_executor_is_dropped() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let runtime = AppRuntime::new(|| {}).expect("runtime builds");
    let spawner = runtime.ui().spawner(owners.clone());

    drop(runtime);

    let error = spawner
        .spawn(owner, async {})
        .expect_err("orphaned tasks must not be accepted");
    assert_eq!(error.to_string(), "UI executor is no longer running");
}

struct TwoPollFuture {
    poll_threads: Rc<RefCell<Vec<thread::ThreadId>>>,
    first_poll: bool,
}

impl Future for TwoPollFuture {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.poll_threads.borrow_mut().push(thread::current().id());
        if self.first_poll {
            self.first_poll = false;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    }
}

struct DropFlag(Rc<Cell<bool>>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.set(true);
    }
}
