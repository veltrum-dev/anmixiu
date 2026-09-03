use std::{cell::Cell, rc::Rc};

use anmixiu_platform_windows::{DisplayCoordinator, PointerPhase, PointerTracker, Viewport};

#[test]
fn click_requires_press_and_release_on_the_same_target() {
    let mut pointer = PointerTracker::default();
    assert!(!pointer.is_inside());
    pointer.update_position(20.0, 30.0);
    assert!(pointer.is_inside());
    assert_eq!(pointer.phase(), PointerPhase::Moving);
    pointer.press(Some(7));
    assert_eq!(pointer.release(Some(8)), None);
    pointer.press(Some(7));
    assert_eq!(pointer.release(Some(7)), Some(7));
    pointer.exit();
    assert!(!pointer.is_inside());
}

#[test]
fn invalidations_are_coalesced_until_the_current_frame_begins() {
    let wakes = Rc::new(Cell::new(0));
    let wakes_for_callback = Rc::clone(&wakes);
    let mut display = DisplayCoordinator::new(move || {
        wakes_for_callback.set(wakes_for_callback.get() + 1);
    });
    assert!(display.invalidate());
    assert!(!display.invalidate());
    assert_eq!(wakes.get(), 1);
    assert!(display.begin_frame());
    display.end_frame(true);
    assert_eq!(display.submission_count(), 1);
}

#[test]
fn exact_physical_backing_size_survives_fractional_logical_dimensions() {
    let viewport = Viewport::with_backing_size(620.25, 520.25, 1.5, 930, 780);
    assert_eq!(viewport.logical_size(), (620.25, 520.25));
    assert_eq!(viewport.physical_size(), (930, 780));
    assert!((viewport.scale() - 1.5).abs() <= f32::EPSILON);
}
