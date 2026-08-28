use std::{cell::Cell, rc::Rc};

use anmixiu_core::px;
use anmixiu_platform_macos::{
    App, DisplayCoordinator, PointerPhase, PointerTracker, Viewport, Window,
};

#[test]
fn click_requires_press_and_release_on_same_topmost_target() {
    let mut pointer = PointerTracker::default();
    pointer.update_position(20.0, 30.0);
    assert_eq!(pointer.phase(), PointerPhase::Moving);
    pointer.press(Some(7));
    assert_eq!(pointer.phase(), PointerPhase::Pressed);
    assert_eq!(pointer.release(Some(8)), None);

    pointer.press(Some(7));
    assert_eq!(pointer.release(Some(7)), Some(7));
    assert_eq!(pointer.phase(), PointerPhase::Released);
}

#[test]
fn frame_requests_are_coalesced_and_clean_frames_do_not_submit() {
    let wakes = Rc::new(Cell::new(0));
    let wakes_for_callback = wakes.clone();
    let mut display = DisplayCoordinator::new(move || {
        wakes_for_callback.set(wakes_for_callback.get() + 1);
    });
    assert!(display.invalidate());
    assert!(!display.invalidate());
    assert_eq!(wakes.get(), 1);
    assert!(display.begin_frame());
    display.end_frame(true);
    assert_eq!(display.submission_count(), 1);
    assert!(!display.begin_frame());
    display.end_frame(false);
    assert_eq!(display.submission_count(), 1);
}

#[test]
fn resize_and_retina_scale_invalidate_only_on_real_change() {
    let mut viewport = Viewport::new(800.0, 600.0, 2.0);
    assert_eq!(viewport.physical_size(), (1600, 1200));
    assert!(!viewport.update(800.0, 600.0, 2.0));
    assert!(viewport.update(640.0, 480.0, 1.0));
    assert_eq!(viewport.physical_size(), (640, 480));
}

#[test]
fn viewport_keeps_the_platform_backing_size_separate_from_logical_layout() {
    let mut viewport = Viewport::with_backing_size(620.25, 520.25, 2.0, 1241, 1041);
    assert_eq!(viewport.logical_size(), (620.25, 520.25));
    assert_eq!(viewport.physical_size(), (1241, 1041));
    assert!((viewport.scale() - 2.0).abs() < f32::EPSILON);

    assert!(!viewport.update_backing_size(620.25, 520.25, 2.0, 1241, 1041));
    assert!(viewport.update_backing_size(620.25, 520.25, 1.0, 620, 520));
    assert_eq!(viewport.physical_size(), (620, 520));
}

#[test]
fn app_and_window_accept_independent_typography_defaults() {
    let window = Window::new()
        .font_family("Window Family")
        .font_size(px(17.0));
    let _app = App::new()
        .font_family("App Family")
        .font_size(px(15.0))
        .window(window);
}
