#![cfg(target_os = "macos")]

use anmixiu_core::{
    AlignItems, Color, Element, GlobalElementId, InteractiveElement, ParentElement, ScrollHandle,
    StatefulInteractiveElement, Styled, button, div, px, text,
};
use anmixiu_platform_macos::FrameBuilder;
use anmixiu_render_metal::{MetalRenderer, SurfaceSize};
use anmixiu_scene::{DrawCommand, Point, Size};

#[test]
fn element_tree_becomes_cached_layout_scene_text_and_hit_regions() {
    let element = div()
        .padding(px(16.0))
        .gap(px(10.0))
        .background(Color::rgb(0.05, 0.07, 0.12))
        .child(text("Hello, 安觅秀"))
        .child(
            button("同步 +1")
                .height(px(44.0))
                .background(Color::rgb(0.2, 0.45, 0.95))
                .foreground(Color::WHITE)
                .id("sync")
                .on_click(|| {}),
        )
        .into_element_node();
    let mut builder = FrameBuilder::new().expect("CoreText initializes");
    let first = builder
        .build(&element, Size::new(480.0, 320.0), 2.0)
        .expect("frame builds");

    assert_eq!(
        first.layout.bounds(first.layout.root()).unwrap().size,
        Size::new(480.0, 320.0)
    );
    assert!(
        first
            .scene
            .commands()
            .iter()
            .any(|command| matches!(command, DrawCommand::Glyphs { .. }))
    );
    assert_eq!(first.scene.hit_regions().len(), 1);
    let button_bounds = first.scene.hit_regions()[0].bounds;
    let hit = first.scene.hit_test(Point::new(
        button_bounds.origin.x + button_bounds.size.width / 2.0,
        button_bounds.origin.y + button_bounds.size.height / 2.0,
    ));
    assert!(hit.is_some());
    assert!(first.handler(hit.unwrap()).is_some());

    let hits_before = builder.layout_cache_stats().hits;
    let second = builder
        .build(&element, Size::new(480.0, 320.0), 2.0)
        .expect("identical frame builds");
    assert!(builder.layout_cache_stats().hits > hits_before);
    assert_eq!(first.layout, second.layout);
}

#[test]
fn stateful_element_identity_survives_positional_sibling_changes() {
    let first_tree = div()
        .child(button("target").id(("item", 9_u64)).on_click(|| {}))
        .into_element_node();
    let second_tree = div()
        .child(text("inserted before target"))
        .child(button("target").id(("item", 9_u64)).on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let first = builder
        .build(&first_tree, Size::new(320.0, 120.0), 1.0)
        .unwrap();
    let first_hit = first.scene.hit_regions()[0].id;
    let first_id = first.global_id(first_hit).cloned().unwrap();
    let second = builder
        .build(&second_tree, Size::new(320.0, 120.0), 1.0)
        .unwrap();
    let second_hit = second.scene.hit_regions()[0].id;

    assert_ne!(first_hit, second_hit, "dense node position changed");
    assert_eq!(second.global_id(second_hit), Some(&first_id));
}

#[test]
fn duplicate_semantic_element_ids_are_rejected() {
    let tree = div()
        .child(button("first").id("duplicate").on_click(|| {}))
        .child(button("second").id("duplicate").on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let error = builder
        .build(&tree, Size::new(320.0, 120.0), 1.0)
        .expect_err("duplicate state keys cannot share a window path");

    assert!(error.to_string().contains("duplicate"));
}

#[test]
fn text_inherits_parent_foreground_when_it_has_no_override() {
    let element = div()
        .foreground(Color::WHITE)
        .child(text("Readable inherited text"))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();
    let glyph_color = frame
        .scene
        .commands()
        .iter()
        .find_map(|command| match command {
            DrawCommand::Glyphs { color, .. } => Some(*color),
            _ => None,
        });
    assert_eq!(glyph_color, Some(anmixiu_scene::Color::WHITE));
}

#[test]
fn backdrop_blur_is_emitted_after_the_backdrop_and_before_the_element_fill() {
    let element = div()
        .background(Color::rgb(1.0, 0.0, 0.0))
        .child(
            div()
                .width(80.0)
                .height(40.0)
                .backdrop_blur(12.0)
                .background(Color::rgba(1.0, 1.0, 1.0, 0.25))
                .rounded(8.0),
        )
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(160.0, 80.0), 2.0)
        .unwrap();

    assert!(matches!(
        frame.scene.commands(),
        [
            DrawCommand::SolidQuad { .. },
            DrawCommand::BackdropBlur {
                sigma,
                corner_radius,
                ..
            },
            DrawCommand::RoundedQuad { .. }
        ] if (*sigma - 12.0).abs() < f32::EPSILON
            && (*corner_radius - 8.0).abs() < f32::EPSILON
    ));
    assert!(frame.scene.requires_compositing());
}

#[test]
fn filter_blur_wraps_the_element_and_its_descendants_but_not_its_backdrop() {
    let element = div()
        .background(Color::rgb(1.0, 0.0, 0.0))
        .child(
            div()
                .width(80.0)
                .height(40.0)
                .filter_blur(10.0)
                .background(Color::WHITE)
                .child(div().width(20.0).height(10.0).background(Color::BLACK)),
        )
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(160.0, 80.0), 2.0)
        .unwrap();

    assert!(matches!(
        frame.scene.commands(),
        [
            DrawCommand::SolidQuad { .. },
            DrawCommand::FilterBlur {
                sigma,
                commands,
                ..
            }
        ] if (*sigma - 10.0).abs() < f32::EPSILON
            && matches!(commands.as_ref(), [
                DrawCommand::SolidQuad { .. },
                DrawCommand::SolidQuad { .. }
            ])
    ));
    assert!(frame.scene.requires_compositing());
}

#[test]
fn changing_only_filter_blur_reuses_layout_and_invalidates_scene_paint() {
    let plain = div()
        .child(div().width(80.0).height(40.0))
        .into_element_node();
    let blurred = div()
        .child(div().width(80.0).height(40.0).filter_blur(10.0))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let first = builder.build(&plain, Size::new(160.0, 80.0), 2.0).unwrap();
    let layout_misses = builder.layout_cache_stats().misses;
    let second = builder
        .build(&blurred, Size::new(160.0, 80.0), 2.0)
        .unwrap();

    assert_eq!(builder.layout_cache_stats().misses, layout_misses);
    assert_eq!(first.layout, second.layout);
    assert_ne!(first.scene, second.scene);
}

#[test]
fn non_positive_and_non_finite_filter_blurs_emit_no_effect() {
    let mut builder = FrameBuilder::new().unwrap();
    for sigma in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let element = div().filter_blur(sigma).into_element_node();
        let frame = builder.build(&element, Size::new(80.0, 40.0), 1.0).unwrap();
        assert!(
            !frame.scene.requires_compositing(),
            "invalid sigma {sigma:?} must keep the direct renderer path"
        );
    }
}

#[test]
fn changing_only_backdrop_blur_reuses_layout_and_invalidates_scene_paint() {
    let plain = div()
        .child(div().width(80.0).height(40.0))
        .into_element_node();
    let blurred = div()
        .child(div().width(80.0).height(40.0).backdrop_blur(16.0))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let first = builder.build(&plain, Size::new(160.0, 80.0), 2.0).unwrap();
    let layout_misses = builder.layout_cache_stats().misses;
    let second = builder
        .build(&blurred, Size::new(160.0, 80.0), 2.0)
        .unwrap();

    assert_eq!(builder.layout_cache_stats().misses, layout_misses);
    assert_eq!(first.layout, second.layout);
    assert_ne!(first.scene, second.scene);
}

#[test]
fn non_positive_and_non_finite_backdrop_blurs_emit_no_effect() {
    let mut builder = FrameBuilder::new().unwrap();
    for sigma in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let element = div().backdrop_blur(sigma).into_element_node();
        let frame = builder.build(&element, Size::new(80.0, 40.0), 1.0).unwrap();
        assert!(
            !frame.scene.requires_compositing(),
            "invalid sigma {sigma:?} must keep the direct renderer path"
        );
    }
}

#[test]
fn hover_changes_only_scene_paint_and_reuses_layout() {
    let element = div()
        .child(button("Hover").id("hover-target").on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let normal = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();
    let layout_misses = builder.layout_cache_stats().misses;
    let target = GlobalElementId::new(["hover-target".into()]);

    assert!(builder.set_hovered(Some(target)));
    let hovered = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();

    assert_eq!(builder.layout_cache_stats().misses, layout_misses);
    assert_eq!(normal.layout, hovered.layout);
    assert_ne!(normal.scene, hovered.scene);
    assert!(matches!(
        normal.scene.commands()[0],
        DrawCommand::RoundedQuad { .. }
    ));
    assert!(matches!(
        normal.scene.commands()[1],
        DrawCommand::RoundedQuad { .. }
    ));
    assert_ne!(normal.scene.commands()[0], hovered.scene.commands()[0]);
    assert_ne!(normal.scene.commands()[1], hovered.scene.commands()[1]);
    assert!(!builder.set_hovered(builder.hovered().cloned()));
}

#[cfg(feature = "devtools")]
#[test]
fn inspected_element_adds_only_a_debug_outline_without_recomputing_layout() {
    let element = div()
        .child(button("Inspect").id("inspect-target").on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let normal = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();
    let layout_misses = builder.layout_cache_stats().misses;
    let target = GlobalElementId::new(["inspect-target".into()]);

    assert!(builder.set_inspected(Some(target.to_string())));
    let inspected = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();

    assert_eq!(builder.layout_cache_stats().misses, layout_misses);
    assert_eq!(normal.layout, inspected.layout);
    assert_eq!(
        inspected.scene.commands().len(),
        normal.scene.commands().len() + 4
    );
}

#[cfg(feature = "devtools")]
#[test]
fn previewed_element_uses_a_transient_debug_outline() {
    let element = div()
        .child(button("Preview").id("preview-target").on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let normal = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();
    assert!(builder.set_previewed(Some(String::from("preview-target"))));
    let previewed = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();

    assert_eq!(
        previewed.scene.commands().len(),
        normal.scene.commands().len() + 4
    );
}

#[cfg(feature = "devtools")]
#[test]
fn previewed_node_can_highlight_a_node_without_a_semantic_id() {
    let element = div().child(text("Preview me")).into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let normal = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();
    assert!(builder.set_previewed_node(Some(1)));
    let previewed = builder
        .build(&element, Size::new(320.0, 80.0), 1.0)
        .unwrap();

    assert_eq!(
        previewed.scene.commands().len(),
        normal.scene.commands().len() + 4
    );
}

#[test]
fn default_button_border_and_hover_render_to_distinct_metal_pixels() {
    let element = div()
        .child(button("Hover").id("hover-target").on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let normal = builder
        .build(&element, Size::new(120.0, 60.0), 1.0)
        .unwrap();
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal unavailable on this macOS host");
        return;
    };
    let size = SurfaceSize::new(120, 60).unwrap();
    let normal_image = renderer.render_offscreen(&normal.scene, size).unwrap();
    let border = normal_image.pixel_rgba(0, 18);
    let background = normal_image.pixel_rgba(2, 18);
    assert_ne!(
        border, background,
        "one-pixel border surrounds the button fill"
    );

    builder.set_hovered(Some(GlobalElementId::new(["hover-target".into()])));
    let hovered = builder
        .build(&element, Size::new(120.0, 60.0), 1.0)
        .unwrap();
    let hovered_image = renderer.render_offscreen(&hovered.scene, size).unwrap();
    assert_ne!(background, hovered_image.pixel_rgba(2, 18));
}

#[test]
fn retina_to_external_scale_transition_preserves_long_button_text_and_clip() {
    let label = "同一事件连续更新 3 次（合并到下一帧）";
    let visible_chars = label
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let element = div()
        .padding(28.0)
        .child(button(label).height(44.0).id("long-button").on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();

    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal unavailable on this macOS host");
        return;
    };
    for scale_integer in [2_u32, 1, 2] {
        let scale = f32::from(u16::try_from(scale_integer).unwrap());
        let frame = builder
            .build(&element, Size::new(620.0, 520.0), scale)
            .unwrap();
        let (glyphs, clip) = frame
            .scene
            .commands()
            .iter()
            .find_map(|command| match command {
                DrawCommand::Glyphs { glyphs, clip, .. } if glyphs.len() > 10 => {
                    Some((glyphs, clip.expect("button text is clipped to its bounds")))
                }
                _ => None,
            })
            .expect("long button glyph run exists");
        assert_eq!(glyphs.len(), visible_chars, "scale {scale} dropped glyphs");
        assert!(
            glyphs
                .iter()
                .all(|glyph| glyph.bounds.max_x() <= clip.bounds.max_x()),
            "scale {scale} measured text wider than its clip"
        );
        let final_bounds = glyphs.last().unwrap().bounds;
        let image = renderer
            .render_offscreen_scaled(
                &frame.scene,
                SurfaceSize::new(620 * scale_integer, 520 * scale_integer).unwrap(),
                scale,
            )
            .unwrap();
        let mut final_glyph_alpha = 0_u64;
        let physical_width = u16::try_from(image.size().width()).unwrap();
        let physical_height = u16::try_from(image.size().height()).unwrap();
        for y in 0..physical_height {
            for x in 0..physical_width {
                let logical_point = Point::new(f32::from(x) / scale, f32::from(y) / scale);
                if final_bounds.contains(logical_point) {
                    final_glyph_alpha += u64::from(image.pixel_rgba(u32::from(x), u32::from(y))[3]);
                }
            }
        }
        assert!(
            final_glyph_alpha > 0,
            "scale {scale} produced an invisible closing punctuation glyph"
        );
    }
}

#[test]
fn final_layout_offset_selects_a_pixel_aligned_glyph_variant() {
    let element = div()
        .align(AlignItems::Center)
        .child(text("Subpixel phase"))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(101.0, 80.0), 1.0)
        .unwrap();
    let glyphs = frame
        .scene
        .commands()
        .iter()
        .find_map(|command| match command {
            DrawCommand::Glyphs { glyphs, .. } => Some(glyphs),
            _ => None,
        })
        .expect("text creates a glyph run");

    assert!(glyphs.iter().all(|glyph| {
        [
            glyph.bounds.origin.x,
            glyph.bounds.origin.y,
            glyph.bounds.size.width,
            glyph.bounds.size.height,
        ]
        .into_iter()
        .all(|edge| (edge - edge.round()).abs() < 0.001)
    }));
}

#[test]
fn button_label_is_centered_and_focus_ring_is_paint_only() {
    let element = button("Centered")
        .width(px(200.0))
        .height(px(100.0))
        .id("centered")
        .on_click(|| {})
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let first = builder
        .build(&element, Size::new(240.0, 120.0), 1.0)
        .unwrap();
    let button_bounds = first.layout.bounds(first.layout.root()).unwrap();
    let glyphs = first
        .scene
        .commands()
        .iter()
        .find_map(|command| match command {
            DrawCommand::Glyphs { glyphs, .. } => Some(glyphs),
            _ => None,
        })
        .expect("button label creates glyphs");
    let min_x = glyphs
        .iter()
        .map(|glyph| glyph.bounds.origin.x)
        .fold(f32::INFINITY, f32::min);
    let max_x = glyphs
        .iter()
        .map(|glyph| glyph.bounds.max_x())
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = glyphs
        .iter()
        .map(|glyph| glyph.bounds.origin.y)
        .fold(f32::INFINITY, f32::min);
    let max_y = glyphs
        .iter()
        .map(|glyph| glyph.bounds.max_y())
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        (min_x.midpoint(max_x) - (button_bounds.origin.x + button_bounds.size.width * 0.5)).abs()
            < 1.0
    );
    assert!(
        (min_y.midpoint(max_y) - (button_bounds.origin.y + button_bounds.size.height * 0.5)).abs()
            < 1.0
    );

    let before = first.scene.commands().len();
    builder.set_focused(Some(GlobalElementId::new(["centered".into()])));
    let focused = builder
        .build(&element, Size::new(240.0, 120.0), 1.0)
        .unwrap();
    assert_eq!(focused.layout, first.layout);
    assert_eq!(focused.scene.commands().len(), before + 1);
    let Some(DrawCommand::RoundedBorder {
        bounds,
        corner_radius,
        border_width,
        clip,
        ..
    }) = focused.scene.commands().last()
    else {
        panic!("focus creates one rounded border command");
    };
    assert_eq!(bounds.origin, Point::new(-2.0, -2.0));
    assert_eq!(bounds.size, Size::new(204.0, 104.0));
    assert!((*corner_radius - 10.0).abs() < f32::EPSILON);
    assert!((*border_width - 2.0).abs() < f32::EPSILON);
    assert!(clip.is_none());
}

#[test]
fn pointer_focus_is_retained_without_showing_the_keyboard_focus_ring() {
    let element = button("Focusable")
        .width(px(120.0))
        .height(px(44.0))
        .id("focus-target")
        .on_click(|| {})
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(160.0, 80.0), 1.0)
        .unwrap();

    assert!(builder.focus_at(&frame, Point::new(10.0, 10.0)));
    assert_eq!(
        builder.focused(),
        Some(&GlobalElementId::new(["focus-target".into()]))
    );
    let pointer_focused = builder
        .build(&element, Size::new(160.0, 80.0), 1.0)
        .unwrap();
    assert_eq!(pointer_focused.scene.commands(), frame.scene.commands());

    assert!(builder.set_focused(Some(GlobalElementId::new(["focus-target".into()]))));
    let keyboard_focused = builder
        .build(&element, Size::new(160.0, 80.0), 1.0)
        .unwrap();
    assert_ne!(
        keyboard_focused.scene.commands(),
        pointer_focused.scene.commands()
    );

    assert!(builder.focus_at(&frame, Point::new(10.0, 10.0)));
    assert!(!builder.focus_at(&frame, Point::new(10.0, 10.0)));
    assert!(builder.focus_at(&frame, Point::new(150.0, 70.0)));
    assert!(builder.focused().is_none());
}

#[test]
fn hover_only_parent_does_not_block_its_clickable_child() {
    let element = div()
        .width(px(120.0))
        .height(px(80.0))
        .hover(|style| style.background(Color::rgb(0.2, 0.3, 0.4)))
        .id("hover-parent")
        .child(
            div()
                .width(px(80.0))
                .height(px(40.0))
                .id("click-child")
                .on_click(|| {}),
        )
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(120.0, 80.0), 1.0)
        .unwrap();
    let point = Point::new(10.0, 10.0);

    let hover = frame
        .hover_target_at(point)
        .expect("hover-only parent is hit");
    assert_eq!(
        frame.global_id(hover),
        Some(&GlobalElementId::new(["hover-parent".into()]))
    );
    let click = frame
        .click_target_at(point)
        .expect("clickable child is hit");
    assert_eq!(
        frame.global_id(click),
        Some(&GlobalElementId::new([
            "hover-parent".into(),
            "click-child".into(),
        ]))
    );
}

#[test]
fn hover_reconciles_when_layout_moves_under_a_stationary_pointer() {
    let transitions = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let first_transitions = transitions.clone();
    let first_element = button("Hover")
        .width(px(100.0))
        .height(px(40.0))
        .id("moving-hover")
        .on_hover_change(move |hovered| first_transitions.borrow_mut().push(hovered))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let first = builder
        .build(&first_element, Size::new(120.0, 120.0), 1.0)
        .unwrap();
    let point = Point::new(10.0, 10.0);
    assert!(builder.update_hover(&first, Some(point)));

    let second_transitions = transitions.clone();
    let moved_element = div()
        .child(div().height(px(60.0)))
        .child(
            button("Hover")
                .width(px(100.0))
                .height(px(40.0))
                .id("moving-hover")
                .on_hover_change(move |hovered| {
                    second_transitions.borrow_mut().push(hovered);
                }),
        )
        .into_element_node();
    let moved = builder
        .build(&moved_element, Size::new(120.0, 120.0), 1.0)
        .unwrap();
    assert!(builder.update_hover(&moved, Some(point)));
    assert!(builder.hovered().is_none());
    assert_eq!(*transitions.borrow(), vec![true, false]);
}

#[test]
fn builtin_button_hover_feedback_does_not_require_an_explicit_id() {
    let element = button("Hover without id")
        .width(px(140.0))
        .height(px(44.0))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let normal = builder
        .build(&element, Size::new(160.0, 80.0), 1.0)
        .unwrap();
    assert!(builder.update_hover(&normal, Some(Point::new(10.0, 10.0))));
    let hovered = builder
        .build(&element, Size::new(160.0, 80.0), 1.0)
        .unwrap();

    assert_ne!(normal.scene, hovered.scene);
}

#[test]
fn handler_presence_invalidates_cached_hit_regions() {
    let passive = div()
        .width(px(100.0))
        .height(px(40.0))
        .id("dynamic-target")
        .into_element_node();
    let interactive = div()
        .width(px(100.0))
        .height(px(40.0))
        .id("dynamic-target")
        .on_click(|| {})
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let before = builder
        .build(&passive, Size::new(120.0, 60.0), 1.0)
        .unwrap();
    assert!(before.click_target_at(Point::new(10.0, 10.0)).is_none());

    let after = builder
        .build(&interactive, Size::new(120.0, 60.0), 1.0)
        .unwrap();
    assert!(after.click_target_at(Point::new(10.0, 10.0)).is_some());
}

#[test]
fn button_defaults_to_intrinsic_width_in_a_column() {
    let element = div()
        .width(px(300.0))
        .child(button("Intrinsic").id("intrinsic").on_click(|| {}))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(300.0, 100.0), 1.0)
        .unwrap();
    let button_bounds = frame
        .layout
        .bounds(anmixiu_layout_taffy::LayoutNodeId(1))
        .unwrap();
    assert!(button_bounds.size.width < 300.0);
    assert!(button_bounds.size.width > 0.0);
}

#[test]
fn scroll_container_supports_smooth_two_axis_offsets_and_publishes_metrics() {
    let handle = ScrollHandle::new();
    let element = div()
        .width(px(220.0))
        .height(px(140.0))
        .background(Color::rgb(0.05, 0.07, 0.12))
        .scroll(&handle)
        .child(
            div()
                .width(px(720.0))
                .height(px(520.0))
                .min_width(px(720.0))
                .min_height(px(520.0))
                .padding(px(12.0))
                .child(text("A wide and tall surface")),
        )
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let first = builder
        .build(&element, Size::new(220.0, 140.0), 1.0)
        .unwrap();
    let viewport = first.layout.bounds(first.layout.root()).unwrap();
    assert!(first.scroll_at_axes(Point::new(20.0, 20.0), 80.0, 100.0));
    assert!((handle.target_x() - 80.0).abs() < f32::EPSILON);
    assert!((handle.target_y() - 100.0).abs() < f32::EPSILON);
    assert!((handle.offset_x() - 0.0).abs() < f32::EPSILON);
    assert!((handle.offset_y() - 0.0).abs() < f32::EPSILON);
    assert!(first.advance_scroll(1.0 / 60.0));
    assert!(handle.offset_x() > 0.0 && handle.offset_y() > 0.0);

    let glyph_before = first
        .scene
        .commands()
        .iter()
        .find_map(|command| match command {
            DrawCommand::Glyphs { glyphs, .. } => glyphs.first().copied(),
            _ => None,
        })
        .expect("scroll content has a glyph");
    handle.set_offset_x(80.0);
    handle.set_offset_y(100.0);
    builder.note_scrolled();
    let scrolled = builder
        .build(&element, Size::new(220.0, 140.0), 1.0)
        .unwrap();
    let glyph_after = scrolled
        .scene
        .commands()
        .iter()
        .find_map(|command| match command {
            DrawCommand::Glyphs { glyphs, .. } => glyphs.first().copied(),
            _ => None,
        })
        .expect("scrolled content has a glyph");
    assert!((glyph_after.bounds.origin.x - (glyph_before.bounds.origin.x - 80.0)).abs() < 0.01);
    assert!((glyph_after.bounds.origin.y - (glyph_before.bounds.origin.y - 100.0)).abs() < 0.01);

    // The framework does not draw a scrollbar; instead it publishes the measured sizes on the
    // handle so the application can render its own. Viewport is the 220x140 container; content is
    // the 720x520 inner surface.
    let _ = viewport;
    let (viewport_w, viewport_h) = handle.viewport_size();
    let (content_w, content_h) = handle.content_size();
    assert!((viewport_w - 220.0).abs() < 0.5 && (viewport_h - 140.0).abs() < 0.5);
    assert!(content_w >= 720.0 && content_h >= 520.0);
    assert!((handle.max_offset_y() - (content_h - viewport_h)).abs() < 0.01);
    assert!((handle.max_offset_x() - (content_w - viewport_w)).abs() < 0.01);
}

#[test]
fn advance_scroll_steps_every_animating_region_in_one_frame() {
    // Two sibling scroll containers, both flung. `advance_scroll` must step BOTH each frame — a
    // short-circuiting `any` would freeze whichever region comes after the first still-animating
    // one.
    let top = ScrollHandle::new();
    let bottom = ScrollHandle::new();
    let scroller = |handle: &ScrollHandle| {
        div()
            .width(px(200.0))
            .height(px(100.0))
            .scroll(handle)
            .child(
                // min_height stops flex from shrinking the tall content to fit the viewport, which
                // would zero the scroll range.
                div()
                    .width(px(200.0))
                    .height(px(600.0))
                    .min_height(px(600.0))
                    .child(text("tall")),
            )
            .into_element_node()
    };
    let element = div()
        .child(scroller(&top))
        .child(scroller(&bottom))
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(200.0, 200.0), 1.0)
        .unwrap();

    // Fling both (target set, offset still 0 until advanced).
    assert!(frame.scroll_at_axes(Point::new(20.0, 20.0), 0.0, 200.0));
    assert!(frame.scroll_at_axes(Point::new(20.0, 150.0), 0.0, 200.0));
    assert!((top.offset_y() - 0.0).abs() < f32::EPSILON);
    assert!((bottom.offset_y() - 0.0).abs() < f32::EPSILON);

    assert!(frame.advance_scroll(1.0 / 60.0));
    // The bug: only `top` would move; `bottom` would still be at 0.
    assert!(top.offset_y() > 0.0, "first region advanced");
    assert!(
        bottom.offset_y() > 0.0,
        "second region advanced in the same frame"
    );
}

#[test]
fn flex_grow_scroll_view_keeps_vertical_overflow_for_large_lists() {
    let handle = ScrollHandle::new();
    let rows = (0..120).map(|_| div().height(px(42.0)));
    let element = div()
        .padding(px(24.0))
        .gap(px(14.0))
        .child(text("双轴滚动示例"))
        .child(text("说明"))
        .child(
            div().height(px(500.0)).scroll(&handle).child(
                div()
                    .width(px(1_520.0))
                    .min_width(px(1_520.0))
                    .gap(px(8.0))
                    .padding(px(14.0))
                    .children(rows),
            ),
        )
        .into_element_node();
    let mut builder = FrameBuilder::new().unwrap();
    let frame = builder
        .build(&element, Size::new(960.0, 720.0), 1.0)
        .unwrap();
    assert!(
        frame.scroll_at_axes(Point::new(100.0, 400.0), 0.0, 120.0),
        "a flex-growing scroll view must expose vertical overflow"
    );
}
