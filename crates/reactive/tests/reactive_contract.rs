use std::cell::Cell;
use std::rc::Rc;

use anmixiu_reactive::{OwnerRegistry, Signal};

#[test]
fn signal_default_and_clone_share_one_value() {
    let signal = Signal::<u32>::default();
    let clone = signal.clone();

    clone.set(7);

    assert_eq!(signal.get(), 7);
    signal.update(|value| *value += 2);
    assert_eq!(clone.get(), 9);
}

#[test]
fn explicit_observation_tracks_reads_and_deduplicates_dirty_owners() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let signal = Signal::new(1_u32);

    assert_eq!(owners.observe(owner, || signal.get()), Some(1));
    assert_eq!(owners.stats().subscription_count, 1);

    signal.set(2);
    signal.set(3);
    signal.update(|value| *value += 1);

    assert_eq!(owners.dirty_len(), 1);
    assert_eq!(owners.take_dirty(), vec![owner]);
    assert!(owners.take_dirty().is_empty());
}

#[test]
fn reads_outside_observation_and_unrelated_signals_do_not_subscribe() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let tracked = Signal::new(10_u32);
    let unrelated = Signal::new(20_u32);

    assert_eq!(unrelated.get(), 20);
    assert_eq!(owners.observe(owner, || tracked.get()), Some(10));

    unrelated.set(21);
    assert!(owners.take_dirty().is_empty());
    assert_eq!(unrelated.subscriber_count(), 0);
    assert_eq!(tracked.subscriber_count(), 1);
}

#[test]
fn a_new_observation_replaces_stale_dependencies() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let first = Signal::new(1_u32);
    let second = Signal::new(2_u32);

    assert_eq!(owners.observe(owner, || first.get()), Some(1));
    assert_eq!(owners.observe(owner, || second.get()), Some(2));

    first.set(3);
    assert!(owners.take_dirty().is_empty());
    second.set(4);
    assert_eq!(owners.take_dirty(), vec![owner]);
    assert_eq!(first.subscriber_count(), 0);
    assert_eq!(second.subscriber_count(), 1);
}

#[test]
fn removing_an_owner_detaches_subscriptions_and_runs_cleanup_once() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let signal = Signal::new(1_u32);
    let cleanup_count = Rc::new(Cell::new(0));
    let cleanup_count_for_callback = cleanup_count.clone();

    assert!(owners.register_cleanup(owner, move || {
        cleanup_count_for_callback.set(cleanup_count_for_callback.get() + 1);
    }));
    assert_eq!(owners.observe(owner, || signal.get()), Some(1));
    assert!(owners.remove_owner(owner));
    assert!(!owners.remove_owner(owner));

    assert_eq!(cleanup_count.get(), 1);
    assert_eq!(signal.subscriber_count(), 0);
    assert_eq!(owners.stats().live_owner_count, 0);
    assert_eq!(owners.stats().subscription_count, 0);
    signal.set(9);
    assert!(owners.take_dirty().is_empty());
}

#[test]
fn repeated_renders_and_mount_cycles_have_bounded_tracking_state() {
    let owners = OwnerRegistry::new();
    let signal = Signal::new(0_u32);

    for _ in 0..1_000 {
        let owner = owners.create_owner();
        for _ in 0..20 {
            assert_eq!(owners.observe(owner, || signal.get()), Some(0));
            assert_eq!(owners.stats().subscription_count, 1);
        }
        assert!(owners.remove_owner(owner));
        assert_eq!(owners.stats().subscription_count, 0);
        assert_eq!(signal.subscriber_count(), 0);
    }

    let stats = owners.stats();
    assert_eq!(stats.live_owner_count, 0);
    assert_eq!(stats.subscription_count, 0);
    assert_eq!(stats.dirty_owner_count, 0);
}

#[test]
fn set_deduplicates_unchanged_values_but_set_always_forces_notify() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let signal = Signal::new(1_u32);
    assert_eq!(owners.observe(owner, || signal.get()), Some(1));

    // Setting the value it already holds must not schedule a frame.
    signal.set(1);
    assert_eq!(owners.dirty_len(), 0, "set(same) is a no-op");

    // A real change notifies.
    signal.set(2);
    assert_eq!(owners.dirty_len(), 1);
    assert_eq!(owners.take_dirty(), vec![owner]);

    // set_always notifies even when the value is unchanged (event/ping semantics).
    signal.set_always(2);
    assert_eq!(owners.dirty_len(), 1, "set_always ignores equality");
    assert_eq!(owners.take_dirty(), vec![owner]);
}

#[test]
fn update_if_changed_only_notifies_on_a_real_change() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    let signal = Signal::new(5_u32);
    assert_eq!(owners.observe(owner, || signal.get()), Some(5));

    // A no-op mutation reports false and schedules nothing.
    assert!(!signal.update_if_changed(|value| *value = 5));
    assert_eq!(owners.dirty_len(), 0);

    // A real mutation reports true and notifies.
    assert!(signal.update_if_changed(|value| *value += 1));
    assert_eq!(signal.get(), 6);
    assert_eq!(owners.dirty_len(), 1);
    assert_eq!(owners.take_dirty(), vec![owner]);
}

#[test]
fn animation_requests_mark_dirty_and_are_taken_per_turn() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();

    assert!(owners.request_animation_frame(owner));
    assert_eq!(owners.animating_len(), 1);
    // Requesting an animation frame also enqueues the owner for render.
    assert_eq!(owners.dirty_len(), 1);
    // A snapshot must not drain the dirty queue (the frame is still pending).
    assert_eq!(owners.dirty_snapshot(), vec![owner]);
    assert_eq!(owners.dirty_len(), 1);

    // Taking the animation set consumes it; the dirty queue is independent.
    assert_eq!(owners.take_animating(), vec![owner]);
    assert_eq!(owners.animating_len(), 0);
    assert_eq!(owners.dirty_len(), 1, "take_animating leaves the dirty mark");
    assert_eq!(owners.take_dirty(), vec![owner]);
}

#[test]
fn animation_requests_record_the_call_site() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();

    let here = std::panic::Location::caller();
    assert!(owners.request_animation_frame(owner));

    let sites = owners.take_animating_with_sites();
    assert_eq!(sites.len(), 1);
    let (recorded_owner, site) = sites[0];
    assert_eq!(recorded_owner, owner);
    // The recorded site is the call above, in this test file — not inside the reactive crate.
    assert!(
        site.file().ends_with("reactive_contract.rs"),
        "recorded call site should be user code, got {}",
        site.file()
    );
    // Same file and adjacent line as the `here` marker captured just before the call.
    assert_eq!(site.file(), here.file());
}

#[test]
fn dead_owner_cannot_request_animation_and_unmount_clears_it() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    assert!(owners.request_animation_frame(owner));
    assert_eq!(owners.stats().animating_owner_count, 1);

    // Removing the owner silences its pending animation request.
    assert!(owners.remove_owner(owner));
    assert_eq!(owners.animating_len(), 0);
    assert_eq!(owners.stats().animating_owner_count, 0);

    // A dead owner cannot re-arm an animation.
    assert!(!owners.request_animation_frame(owner));
    assert_eq!(owners.animating_len(), 0);
}

#[test]
fn set_runs_the_previous_values_drop_outside_the_borrow() {
    // A value whose Drop reads the same signal must not hit an "already borrowed" panic: `set`
    // releases the value borrow before dropping the replaced value.
    thread_local! {
        static PROBE: Signal<Option<ReadsOnDrop>> = Signal::new(None);
    }

    struct ReadsOnDrop;
    impl Drop for ReadsOnDrop {
        fn drop(&mut self) {
            // Re-enter the signal with a read while it is mid-`set`.
            PROBE.with(|signal| {
                let _ = signal.with(Option::is_some);
            });
        }
    }

    PROBE.with(|signal| {
        // ReadsOnDrop is not PartialEq, so this exercises the general `set_always` path.
        signal.set_always(Some(ReadsOnDrop));
        // Replacing the value drops the previous ReadsOnDrop, which reads the signal from Drop.
        signal.set_always(None);
    });
}

#[test]
fn invalid_owners_cannot_observe_or_be_marked_dirty() {
    let owners = OwnerRegistry::new();
    let owner = owners.create_owner();
    assert!(owners.remove_owner(owner));

    let signal = Signal::new(1_u32);
    assert_eq!(owners.observe(owner, || signal.get()), None);
    assert!(!owners.mark_dirty(owner));
    assert!(!owners.register_cleanup(owner, || panic!("must not register")));
}
