use std::{
    cell::{Cell, RefCell},
    future::{pending, ready},
    rc::Rc,
};

use anmixiu_core::{
    AppEvents, AppStateStore, Color, ComponentHost, Context, CursorStyle, Element, ElementId,
    ElementNode, EventPriority, EventScope, Eventful, FluentBuilder, FrameBatcher,
    InteractiveElement, NodeId, ParentElement, Pixels, Render, RenderOnce, SharedString, State,
    StatefulInteractiveElement, Styled, Typography, WindowId, WindowStateStore, button, div, px,
    shared_format, text,
};
use anmixiu_reactive::OwnerRegistry;
use anmixiu_reactive::Signal;
use anmixiu_runtime::AppRuntime;

#[derive(Default)]
struct LifecycleProbe {
    renders: Cell<usize>,
    mounts: Cell<usize>,
    unmounts: Cell<usize>,
}

impl Render for LifecycleProbe {
    fn on_mount(&self, _cx: &mut Context<Self>) {
        self.mounts.set(self.mounts.get() + 1);
    }

    fn render(&self, _cx: &mut Context<Self>) -> impl anmixiu_core::IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().child(text("probe"))
    }

    fn on_unmount(&self, _cx: &mut Context<Self>) {
        self.unmounts.set(self.unmounts.get() + 1);
    }
}

#[test]
fn render_borrows_component_and_lifecycle_runs_once_after_first_paint() {
    let probe = Rc::new(LifecycleProbe::default());
    let mut host = ComponentHost::new(probe.clone(), Context::testing());

    host.render().expect("initial render");
    assert_eq!(probe.renders.get(), 1);
    assert_eq!(probe.mounts.get(), 0, "mount is post-paint");
    host.did_paint();
    host.did_paint();
    assert_eq!(probe.mounts.get(), 1);

    host.unmount();
    host.unmount();
    assert_eq!(probe.unmounts.get(), 1);
}

struct OneShot(Cell<bool>);

impl RenderOnce for OneShot {
    fn render(self, _cx: &mut Context<Self>) -> impl anmixiu_core::IntoElement {
        self.0.set(true);
        text("once")
    }
}

#[test]
fn render_once_consumes_value_and_has_no_lifecycle() {
    let rendered = Cell::new(false);
    let element =
        ComponentHost::<LifecycleProbe>::render_once(OneShot(rendered), Context::testing());
    assert_eq!(element.kind_name(), "text");
}

#[derive(Default)]
struct AppState {
    name: String,
}

#[derive(Default)]
struct WindowState;

#[test]
fn state_prefers_window_then_falls_back_to_application_and_releases() {
    let app = AppStateStore::new().with(AppState { name: "app".into() });
    let weak_app = app.weak::<AppState>().unwrap();
    let window = WindowStateStore::new().with(AppState {
        name: "window".into(),
    });
    let weak_window = window.weak::<AppState>().unwrap();
    let cx = Context::<LifecycleProbe>::testing_with_state(app.clone(), window.clone());
    let State(value) = cx.state::<AppState>();
    assert_eq!(value.name, "window");
    assert!(cx.try_state::<WindowState>().is_none());

    drop(value);
    drop(cx);
    drop(window);
    assert!(weak_window.upgrade().is_none());
    assert!(weak_app.upgrade().is_some());
    drop(app);
    assert!(weak_app.upgrade().is_none());
}

#[test]
#[should_panic(expected = "public_contract::WindowState")]
fn missing_state_diagnostic_contains_type_name() {
    let cx = Context::<LifecycleProbe>::testing();
    let _ = cx.state::<WindowState>();
}

#[test]
fn builders_keep_long_names_and_click_adapter_accepts_sync_and_async() {
    let sync = button("sync")
        .width(px(100.0))
        .height(px(40.0))
        .padding(px(8.0))
        .gap(px(4.0))
        .id("sync")
        .on_click(|| {});
    let asynchronous = button("async")
        .id(("async", 7_u64))
        .on_click(|| async { ready(()).await });
    assert_eq!(sync.style_ref().width, Some(px(100.0)));
    assert!(sync.click_handler().is_some());
    assert!(asynchronous.click_handler().is_some());
}

#[test]
fn hover_handler_receives_enter_and_leave_transitions() {
    let transitions = Rc::new(RefCell::new(Vec::new()));
    let capture = transitions.clone();
    let node = button("hover")
        .id("hover")
        .on_hover_change(move |hovered| capture.borrow_mut().push(hovered))
        .into_element_node();
    let handler = node.hover_handler().cloned().expect("hover handler");

    handler.invoke(true);
    handler.invoke(false);
    assert_eq!(*transitions.borrow(), vec![true, false]);
}

#[test]
fn element_id_is_semantic_and_survives_stateful_type_erasure() {
    let first = button("first").id(("row", 42_u64)).into_element_node();
    let second = button("second").id(("row", 42_u64)).into_element_node();
    let expected = ElementId::from(("row", 42_u64));

    assert_eq!(first.element_id(), Some(&expected));
    assert_eq!(second.element_id(), Some(&expected));
    assert_eq!(first.element_id(), second.element_id());
}

#[test]
fn shared_string_static_and_heap_clones_reuse_their_bytes() {
    let literal = "static-label";
    let static_value = SharedString::new_static(literal);
    assert_eq!(static_value.as_ptr(), literal.as_ptr());

    let long = SharedString::from("a label that is deliberately longer than twenty-three bytes");
    let cloned = long.clone();
    assert_eq!(long.as_ptr(), cloned.as_ptr());

    let formatted = shared_format!("Count {}", 42);
    assert_eq!(formatted.as_str(), "Count 42");
    assert!(!formatted.is_heap_allocated());
}

#[test]
fn fluent_conditionals_preserve_the_builder_type_and_only_apply_selected_branches() {
    let optional = Some("optional");
    let element = div()
        .when(true, |this| this.child(text("when")))
        .when(false, |this| this.child(text("skipped")))
        .when_else(
            true,
            |this| this.child(text("then")),
            |this| this.child(text("else")),
        )
        .when_some(optional, |this, value| this.child(text(value)))
        .when_none(&Some("present"), |this| this.child(text("none")));

    let labels: Vec<_> = element
        .children_ref()
        .iter()
        .filter_map(|child| child.text_content())
        .collect();
    assert_eq!(labels, ["when", "then", "optional"]);
}

struct Badge(SharedString);

impl Element for Badge {
    fn into_element_node(self) -> ElementNode {
        div()
            .padding(px(4.0))
            .child(text(self.0))
            .into_element_node()
    }
}

#[test]
fn custom_elements_implement_element_not_component_render() {
    let tree = div().child(Badge(SharedString::new_static("custom")));
    assert_eq!(tree.children_ref()[0].kind_name(), "div");
    assert_eq!(
        tree.children_ref()[0].children_ref()[0].text_content(),
        Some("custom")
    );
}

fn styled_card<E: Styled>(element: E) -> E {
    element.width(px(120.0)).padding(px(8.0))
}

#[test]
fn style_and_parent_builders_are_trait_capabilities() {
    let element = styled_card(div())
        .child(text("child"))
        .id("card")
        .on_click(|| {});

    assert_eq!(element.style_ref().width, Some(px(120.0)));
    assert_eq!(element.children_ref().len(), 1);
}

#[derive(Clone, Copy)]
struct Ping;

#[derive(Clone, Copy)]
struct FollowUp;

#[test]
fn event_subscriptions_dispatch_by_priority_then_registration_order() {
    let cx = Context::<LifecycleProbe>::testing();
    let calls = Rc::new(RefCell::new(Vec::new()));

    let first = calls.clone();
    let _normal = cx.subscribe::<Ping, _>(EventScope::App, EventPriority::NORMAL, move |_| {
        first.borrow_mut().push("normal-first");
    });
    let second = calls.clone();
    let _high = cx.subscribe::<Ping, _>(EventScope::App, EventPriority::HIGH, move |_| {
        second.borrow_mut().push("high");
    });
    let third = calls.clone();
    let _normal_second =
        cx.subscribe::<Ping, _>(EventScope::App, EventPriority::NORMAL, move |_| {
            third.borrow_mut().push("normal-second");
        });

    cx.emit(Ping, EventScope::App).expect("event dispatch");
    assert_eq!(
        *calls.borrow(),
        vec!["high", "normal-first", "normal-second"]
    );
}

#[test]
fn nested_events_are_queued_fifo_without_reentrant_router_borrowing() {
    let cx = Context::<LifecycleProbe>::testing();
    let events = cx.event_context();
    let calls = Rc::new(RefCell::new(Vec::new()));

    let follow_calls = calls.clone();
    let _follow = cx.subscribe::<FollowUp, _>(EventScope::App, 0, move |_| {
        follow_calls.borrow_mut().push("follow-up");
    });
    let ping_calls = calls.clone();
    let nested_events = events.clone();
    let _ping = cx.subscribe::<Ping, _>(EventScope::App, 0, move |_| {
        ping_calls.borrow_mut().push("ping");
        nested_events
            .emit(FollowUp, EventScope::App)
            .expect("nested event dispatch");
    });

    events.emit(Ping, EventScope::App).expect("event dispatch");
    assert_eq!(*calls.borrow(), vec!["ping", "follow-up"]);
}

#[test]
fn event_scopes_route_window_and_app_events() {
    let events = AppEvents::new();
    let first = Context::<LifecycleProbe>::testing_with_state_and_events(
        AppStateStore::new(),
        WindowStateStore::new(),
        events.clone(),
        WindowId::new(1),
    );
    let second = Context::<LifecycleProbe>::testing_with_state_and_events(
        AppStateStore::new(),
        WindowStateStore::new(),
        events,
        WindowId::new(2),
    );
    let calls = Rc::new(RefCell::new(Vec::new()));
    let first_calls = calls.clone();
    let _first_window = first.subscribe::<Ping, _>(EventScope::Window, 0, move |_| {
        first_calls.borrow_mut().push("window-1");
    });
    let second_calls = calls.clone();
    let _second_window = second.subscribe::<Ping, _>(EventScope::Window, 0, move |_| {
        second_calls.borrow_mut().push("window-2");
    });
    let app_calls = calls.clone();
    let _first_app = first.subscribe::<Ping, _>(EventScope::App, 0, move |_| {
        app_calls.borrow_mut().push("app-1");
    });

    let first_subscriptions = first.event_context().subscriptions();
    assert_eq!(first_subscriptions.len(), 2);
    assert_eq!(first_subscriptions[0].scope, EventScope::Window);
    assert_eq!(first_subscriptions[1].scope, EventScope::App);
    assert_eq!(first_subscriptions[0].priority, EventPriority::NORMAL);

    first.emit(Ping, EventScope::Window).expect("window event");
    assert_eq!(*calls.borrow(), vec!["window-1"]);
    calls.borrow_mut().clear();

    second.emit(Ping, EventScope::App).expect("app event");
    assert_eq!(*calls.borrow(), vec!["app-1"]);
}

#[test]
fn unmount_removes_event_subscriptions() {
    let cx = Context::<LifecycleProbe>::testing();
    let events = cx.event_context();
    let _subscription = cx.subscribe::<Ping, _>(EventScope::App, 0, |_| {});
    assert_eq!(events.subscription_count(), 1);

    let registry = cx.owner_registry().clone();
    assert!(registry.remove_owner(cx.owner_id()));
    assert_eq!(events.subscription_count(), 0);
}

#[derive(Default)]
struct EventfulProbe {
    count: Signal<u32>,
}

impl Eventful for EventfulProbe {
    fn bind_events(&self, _cx: &mut Context<Self>, bindings: &mut anmixiu_core::EventBindings) {
        let count = self.count.clone();
        bindings.subscribe::<Ping, _>(EventScope::App, 0, move |_| {
            count.update(|value| *value += 1);
        });
    }
}

impl Render for EventfulProbe {
    fn render(&self, _cx: &mut Context<Self>) -> impl anmixiu_core::IntoElement {
        text(self.count.get().to_string())
    }
}

#[test]
fn eventful_capability_binds_once_when_host_paints() {
    let events = AppEvents::new();
    let context = Context::<EventfulProbe>::testing_with_state_and_events(
        AppStateStore::new(),
        WindowStateStore::new(),
        events,
        WindowId::new(3),
    );
    let emitter = context.event_context();
    let probe = Rc::new(EventfulProbe::default());
    let mut host = ComponentHost::new_eventful(probe.clone(), context);

    host.render().expect("initial render");
    host.did_paint();
    host.did_paint();
    emitter.emit(Ping, EventScope::App).expect("event dispatch");
    assert_eq!(probe.count.get(), 1);
    assert_eq!(host.reactive_stats().live_owner_count, 1);
    host.unmount();
    assert_eq!(emitter.subscription_count(), 0);
}

#[test]
fn builtin_button_is_visible_and_usable_without_appearance_boilerplate() {
    let default_button = button("Save");
    let style = default_button.style_ref();

    assert!(style.background.alpha > 0.0, "button background is visible");
    assert!(
        style.foreground.is_some(),
        "button label has a readable foreground"
    );
    assert!(
        style.min_height.is_some(),
        "button has a usable hit-target height"
    );
    assert!(style.padding.value() > 0.0);
    assert!(style.border_radius.value() > 0.0);
    assert!(style.border_width.value() > 0.0);
    assert!(style.border_color.alpha > 0.0);
    assert!(default_button.hover_style().is_some());
    assert_eq!(style.align_self, Some(anmixiu_core::AlignItems::Start));
    assert_eq!(style.cursor, CursorStyle::Pointer);
    assert!(style.focus_ring_color.is_some());
    assert!(style.focus_ring_width.value() > 0.0);

    let custom = default_button.background(Color::rgb(1.0, 0.0, 0.0));
    assert_eq!(custom.style_ref().background, Color::rgb(1.0, 0.0, 0.0));
}

#[test]
fn container_and_text_defaults_remain_neutral_and_composable() {
    let container = div();
    let text = text("Readable");

    assert_eq!(
        container.style_ref().flex_direction,
        anmixiu_core::FlexDirection::Column
    );
    assert_eq!(container.style_ref().background, Color::TRANSPARENT);
    assert_eq!(container.style_ref().border_width, px(0.0));
    assert_eq!(
        text.style_ref().foreground,
        None,
        "text inherits foreground"
    );
    assert_eq!(text.style_ref().background, Color::TRANSPARENT);
}

#[test]
fn colors_support_const_hex_rgb_and_rgba_literals() {
    const BLUE: Color = Color::hex(0x33_66_FF);
    const TRANSLUCENT_BLUE: Color = Color::hex_with_alpha(0x33_66_FF_80);

    assert_eq!(
        BLUE,
        Color::rgba(
            f32::from(0x33_u8) / 255.0,
            f32::from(0x66_u8) / 255.0,
            1.0,
            1.0,
        )
    );
    assert_eq!(
        TRANSLUCENT_BLUE,
        Color::rgba(
            f32::from(0x33_u8) / 255.0,
            f32::from(0x66_u8) / 255.0,
            1.0,
            f32::from(0x80_u8) / 255.0,
        )
    );
}

#[test]
fn color_builders_accept_unambiguous_rgb_integer_literals_via_into() {
    let element = div()
        .background(0x12_34_56)
        .foreground(0xFF_FF_FF)
        .border_color(0x65_43_21);
    assert_eq!(element.style_ref().background, Color::hex(0x12_34_56));
    assert_eq!(element.style_ref().foreground, Some(Color::hex(0xFF_FF_FF)));
    assert_eq!(element.style_ref().border_color, Color::hex(0x65_43_21));

    let hovered = button("Hover").hover(|style| {
        style
            .background(0x22_33_44)
            .foreground(0xEE_EE_EE)
            .border_color(0x77_88_99)
    });
    let hover = hovered.hover_style().unwrap();
    assert_eq!(hover.background, Some(Color::hex(0x22_33_44)));
    assert_eq!(hover.foreground, Some(Color::hex(0xEE_EE_EE)));
    assert_eq!(hover.border_color, Some(Color::hex(0x77_88_99)));
}

#[test]
fn px_returns_pixels_and_builders_preserve_the_concrete_unit() {
    let explicit: Pixels = px(24.0);
    assert!((explicit.value() - 24.0).abs() < f32::EPSILON);

    let element = div()
        .width(320.0)
        .height(200)
        .min_width(120.0)
        .max_height(480)
        .padding(16.0)
        .gap(12)
        .rounded(8.0)
        .border_width(1);
    let style = element.style_ref();

    assert_eq!(style.width, Some(px(320.0)));
    assert_eq!(style.height, Some(px(200.0)));
    assert_eq!(style.min_width, Some(px(120.0)));
    assert_eq!(style.max_height, Some(px(480.0)));
    assert_eq!(style.padding, px(16.0));
    assert_eq!(style.gap, px(12.0));
    assert_eq!(style.border_radius, px(8.0));
    assert_eq!(style.border_width, px(1.0));
}

#[test]
fn window_typography_overrides_app_fields_independently_and_preserves_platform_default() {
    let app = Typography::new()
        .with_font_family("App Family")
        .with_font_size(px(15.0));
    let window = Typography::new().with_font_size(px(17.0));
    let resolved = window.with_fallback(&app);

    assert_eq!(
        resolved.font_family().map(SharedString::as_str),
        Some("App Family")
    );
    assert_eq!(resolved.font_size(), Some(px(17.0)));

    let platform_default = Typography::new().with_fallback(&Typography::new());
    assert!(platform_default.font_family().is_none());
    assert!(platform_default.font_size().is_none());
}

#[test]
#[should_panic(expected = "0xRRGGBB")]
fn rgb_hex_rejects_values_wider_than_24_bits() {
    let _ = Color::hex(0x01_00_00_00);
}

#[test]
#[should_panic(expected = "0xRRGGBB")]
fn integer_into_color_rejects_implicit_alpha() {
    let _ = div().background(0x33_66_FF_80);
}

#[test]
fn hover_refinement_is_an_interaction_capability_and_keeps_layout_fields_out() {
    let button = button("Hover")
        .hover(|style| {
            style
                .background(Color::rgb(0.4, 0.4, 0.5))
                .border_color(Color::WHITE)
        })
        .id("hover");
    let hover = button.hover_style().unwrap();

    assert_eq!(hover.background, Some(Color::rgb(0.4, 0.4, 0.5)));
    assert_eq!(hover.border_color, Some(Color::WHITE));
}

#[test]
fn hit_testing_uses_layout_bounds_and_topmost_clickable_node() {
    let tree = div()
        .child(button("back").width(px(100.0)).height(px(100.0)))
        .child(button("front").width(px(40.0)).height(px(40.0)))
        .into_element_node();
    let hit = tree.hit_test(20.0, 20.0, |id| match id.index() {
        1 => Some((0.0, 0.0, 100.0, 100.0)),
        2 => Some((10.0, 10.0, 40.0, 40.0)),
        _ => Some((0.0, 0.0, 120.0, 120.0)),
    });
    assert_eq!(hit.map(|node| node.text_content()), Some(Some("front")));
}

#[test]
fn hit_testing_uses_half_open_bounds_matching_the_scene() {
    let tree = div()
        .child(button("target").width(px(40.0)).height(px(30.0)))
        .into_element_node();
    let bounds = |id: NodeId| match id.index() {
        1 => Some((10.0, 20.0, 40.0, 30.0)),
        _ => Some((0.0, 0.0, 200.0, 200.0)),
    };
    // Inside and on the top-left edge: hit.
    assert_eq!(
        tree.hit_test(10.0, 20.0, bounds)
            .and_then(|n| n.text_content()),
        Some("target"),
        "top-left corner"
    );
    assert_eq!(
        tree.hit_test(49.999, 49.999, bounds)
            .and_then(|n| n.text_content()),
        Some("target"),
        "just inside"
    );
    // Exactly on the right/bottom edge belongs to the next pixel, not this element — the
    // rendered scene (`Rect::contains`) treats it the same way.
    assert_ne!(
        tree.hit_test(50.0, 35.0, bounds)
            .and_then(|n| n.text_content()),
        Some("target"),
        "right edge is exclusive"
    );
    assert_ne!(
        tree.hit_test(30.0, 50.0, bounds)
            .and_then(|n| n.text_content()),
        Some("target"),
        "bottom edge is exclusive"
    );
}

#[test]
fn scheduler_forgets_closed_windows_and_unmounted_components() {
    let mut batcher = FrameBatcher::new(3);
    let window = WindowId::new(9);
    batcher.mark_dirty(window, 10, None);
    batcher.mark_dirty(window, 11, Some(10));
    assert_eq!(batcher.frame_requests(window), 1);

    // Forgetting a component clears its pending dirty mark so it is not re-rendered.
    batcher.forget_component(window, 11);
    assert_eq!(batcher.begin_frame(window), vec![10]);

    // Forgetting the window resets all of its accumulated state.
    batcher.forget_window(window);
    assert_eq!(batcher.frame_requests(window), 0);
    assert_eq!(batcher.begin_frame(window), Vec::<u64>::new());
}

#[test]
fn scheduler_batches_writes_per_window_and_defers_render_time_invalidations() {
    let mut batcher = FrameBatcher::new(3);
    let window = WindowId::new(7);
    batcher.mark_dirty(window, 10, None);
    batcher.mark_dirty(window, 10, None);
    batcher.mark_dirty(window, 11, Some(10));
    assert_eq!(batcher.frame_requests(window), 1);
    assert_eq!(batcher.begin_frame(window), vec![10]);

    batcher.mark_dirty(window, 10, None);
    assert!(batcher.finish_frame(window, true));
    assert_eq!(batcher.submissions(window), 1);
    assert_eq!(batcher.begin_frame(window), vec![10]);

    assert!(!batcher.finish_frame(window, false));
    assert_eq!(batcher.submissions(window), 1, "no dirty paint, no submit");
}

#[test]
fn scheduler_loop_guard_reports_pathological_invalidation() {
    let mut batcher = FrameBatcher::new(2);
    let window = WindowId::new(1);
    batcher.mark_dirty(window, 1, None);
    let _ = batcher.begin_frame(window);
    batcher.mark_dirty(window, 1, None);
    assert!(batcher.finish_frame(window, true));
    let _ = batcher.begin_frame(window);
    batcher.mark_dirty(window, 1, None);
    assert!(!batcher.finish_frame(window, true));
    assert!(batcher.take_loop_error(window).is_some());
}

#[derive(Default)]
struct Counter {
    count: Signal<u32>,
    unrelated: Signal<u32>,
}

impl Render for Counter {
    fn render(&self, _cx: &mut Context<Self>) -> impl anmixiu_core::IntoElement {
        text(format!("Count: {}", self.count.get()))
    }
}

#[test]
fn signal_component_rerender_contract_uses_shared_signal_handle() {
    let component = Rc::new(Counter::default());
    let count = component.count.clone();
    assert_eq!(count.get(), 0);
    let mut host = ComponentHost::new(component.clone(), Context::testing());
    host.render().unwrap();
    assert_eq!(count.subscriber_count(), 1);
    assert_eq!(component.unrelated.subscriber_count(), 0);
    component.unrelated.set(9);
    assert_eq!(host.reactive_stats().dirty_owner_count, 0);
    count.update(|value| *value += 1);
    assert_eq!(component.count.get(), 1);
    assert_eq!(host.reactive_stats().dirty_owner_count, 1);
    for _ in 0..100 {
        host.render().unwrap();
    }
    assert_eq!(
        count.subscriber_count(),
        1,
        "rerender replaces dependency edges"
    );
    host.unmount();
    assert_eq!(count.subscriber_count(), 0);
}

struct DropFlag(Rc<Cell<bool>>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

struct SpawnOnMount {
    dropped: Rc<Cell<bool>>,
}

impl Render for SpawnOnMount {
    fn on_mount(&self, cx: &mut Context<Self>) {
        let guard = DropFlag(self.dropped.clone());
        cx.spawn(async move {
            let _guard = guard;
            pending::<()>().await;
        });
    }

    fn render(&self, _cx: &mut Context<Self>) -> impl anmixiu_core::IntoElement {
        text("owner task")
    }
}

#[test]
fn component_owner_cancels_context_tasks_on_unmount() {
    let runtime = AppRuntime::new(|| {}).unwrap();
    let owners = OwnerRegistry::new();
    let spawner = runtime.ui().spawner(owners.clone());
    let context = Context::with_owner_spawner(
        AppStateStore::new(),
        WindowStateStore::new(),
        owners,
        move |owner, future| spawner.spawn(owner, future).unwrap(),
    );
    let dropped = Rc::new(Cell::new(false));
    let component = Rc::new(SpawnOnMount {
        dropped: dropped.clone(),
    });
    let mut host = ComponentHost::new(component, context);
    host.render().unwrap();
    host.did_paint();
    runtime.ui().run_ready().unwrap();
    assert!(!dropped.get());
    host.unmount();
    runtime.ui().run_ready().unwrap();
    assert!(dropped.get());
}

#[test]
fn rendered_element_snapshots_share_the_tree_until_the_next_render() {
    let probe = Rc::new(LifecycleProbe::default());
    let mut host = ComponentHost::new(probe, Context::testing());
    host.render().unwrap();
    let first = host.element_snapshot().unwrap();
    let shared = host.element_snapshot().unwrap();
    assert!(Rc::ptr_eq(&first, &shared));

    host.render().unwrap();
    let second = host.element_snapshot().unwrap();
    assert!(!Rc::ptr_eq(&first, &second));
}
