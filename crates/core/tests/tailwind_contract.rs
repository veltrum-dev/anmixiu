#![cfg(feature = "tailwind")]

use anmixiu_core::{
    AlignItems, Color, CursorStyle, FlexDirection, JustifyContent, StyleRefinement, Styled, div, px,
};

#[test]
fn tailwind_value_aliases_preserve_the_long_form_style_contract() {
    let element = div()
        .w(120)
        .h(80.0)
        .min_w(40)
        .min_h(30.0)
        .max_w(240)
        .max_h(160.0)
        .p(12)
        .grow(2.0)
        .shrink(0.25)
        .items(AlignItems::End)
        .bg(0x12_34_56)
        .text_color(Color::WHITE)
        .border(3)
        .blur(5.0)
        .ring(0x65_43_21, 2);
    let style = element.style_ref();

    assert_eq!(style.width, Some(px(120.0)));
    assert_eq!(style.height, Some(px(80.0)));
    assert_eq!(style.min_width, Some(px(40.0)));
    assert_eq!(style.min_height, Some(px(30.0)));
    assert_eq!(style.max_width, Some(px(240.0)));
    assert_eq!(style.max_height, Some(px(160.0)));
    assert_eq!(style.padding, px(12.0));
    assert_eq!(style.flex_grow.to_bits(), 2.0_f32.to_bits());
    assert_eq!(style.flex_shrink.to_bits(), 0.25_f32.to_bits());
    assert_eq!(style.align_items, AlignItems::End);
    assert_eq!(style.background, Color::hex(0x12_34_56));
    assert_eq!(style.foreground, Some(Color::WHITE));
    assert_eq!(style.border_width, px(3.0));
    assert_eq!(style.filter_blur, Some(px(5.0)));
    assert_eq!(style.focus_ring_color, Some(Color::hex(0x65_43_21)));
    assert_eq!(style.focus_ring_width, px(2.0));
}

#[test]
fn tailwind_enum_aliases_cover_every_supported_variant() {
    assert_eq!(
        div().flex_col().style_ref().flex_direction,
        FlexDirection::Column
    );

    assert_eq!(
        div().items_start().style_ref().align_items,
        AlignItems::Start
    );
    assert_eq!(
        div().items_stretch().style_ref().align_items,
        AlignItems::Stretch
    );
    assert_eq!(
        div().items_center().style_ref().align_items,
        AlignItems::Center
    );
    assert_eq!(div().items_end().style_ref().align_items, AlignItems::End);

    assert_eq!(
        div().self_start().style_ref().align_self,
        Some(AlignItems::Start)
    );
    assert_eq!(
        div().self_stretch().style_ref().align_self,
        Some(AlignItems::Stretch)
    );
    assert_eq!(
        div().self_center().style_ref().align_self,
        Some(AlignItems::Center)
    );
    assert_eq!(
        div().self_end().style_ref().align_self,
        Some(AlignItems::End)
    );

    assert_eq!(
        div().justify_start().style_ref().justify_content,
        JustifyContent::Start
    );
    assert_eq!(
        div().justify_center().style_ref().justify_content,
        JustifyContent::Center
    );
    assert_eq!(
        div().justify_end().style_ref().justify_content,
        JustifyContent::End
    );
    assert_eq!(
        div().justify_between().style_ref().justify_content,
        JustifyContent::SpaceBetween
    );

    assert_eq!(
        div().cursor_default().style_ref().cursor,
        CursorStyle::Default
    );
    assert_eq!(
        div().cursor_pointer().style_ref().cursor,
        CursorStyle::Pointer
    );
    assert_eq!(div().cursor_text().style_ref().cursor, CursorStyle::Text);
}

#[test]
fn tailwind_paint_refinement_aliases_match_element_aliases() {
    let refinement = StyleRefinement::default()
        .bg(0x12_34_56)
        .text_color(Color::WHITE);

    assert_eq!(refinement.background, Some(Color::hex(0x12_34_56)));
    assert_eq!(refinement.foreground, Some(Color::WHITE));
}
