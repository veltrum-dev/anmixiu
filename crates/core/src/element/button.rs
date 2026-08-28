use crate::SharedString;

use super::{
    id::ElementId,
    interaction::{ClickHandler, HoverHandler, InteractiveElement},
    node::{ElementBase, ElementKind, ElementNode},
    style::{AlignItems, Color, CursorStyle, Style, StyleRefinement, Styled, px},
    traits::Element,
};

/// Push-button element returned by [`button`].
#[derive(Clone, Debug)]
pub struct ButtonElement {
    base: ElementBase,
    label: SharedString,
}

impl Styled for ButtonElement {
    fn style(&mut self) -> &mut Style {
        &mut self.base.style
    }

    fn style_ref(&self) -> &Style {
        &self.base.style
    }
}

impl InteractiveElement for ButtonElement {
    fn assign_element_id(&mut self, id: ElementId) {
        self.base.id = Some(id);
    }

    fn element_id(&self) -> Option<&ElementId> {
        self.base.id.as_ref()
    }

    fn assign_click_handler(&mut self, handler: ClickHandler) {
        self.base.click = Some(handler);
    }

    fn click_handler(&self) -> Option<&ClickHandler> {
        self.base.click.as_ref()
    }

    fn assign_hover_style(&mut self, style: StyleRefinement) {
        self.base.hover = Some(style);
    }

    fn hover_style(&self) -> Option<&StyleRefinement> {
        self.base.hover.as_ref()
    }

    fn assign_hover_handler(&mut self, handler: HoverHandler) {
        self.base.hover_handler = Some(handler);
    }

    fn hover_handler(&self) -> Option<&HoverHandler> {
        self.base.hover_handler.as_ref()
    }
}

impl Element for ButtonElement {
    fn into_element_node(self) -> ElementNode {
        ElementNode::new(ElementKind::Button(self.label), self.base, Vec::new())
    }
}

#[must_use]
pub fn button(label: impl Into<SharedString>) -> ButtonElement {
    let mut base = ElementBase::default();
    base.style.padding = px(10.0);
    base.style.min_height = Some(px(36.0));
    base.style.background = Color::hex(0x29_33_47);
    base.style.foreground = Some(Color::WHITE);
    base.style.border_width = px(1.0);
    base.style.border_color = Color::hex(0x52_61_7A);
    base.style.border_radius = px(8.0);
    base.style.align_self = Some(AlignItems::Start);
    base.style.cursor = CursorStyle::Pointer;
    base.style.focus_ring_color = Some(Color::hex(0x7A_B8_FF));
    base.style.focus_ring_width = px(2.0);
    base.hover = Some(
        StyleRefinement::default()
            .background(Color::hex(0x38_45_5C))
            .border_color(Color::hex(0x7A_94_B8)),
    );
    ButtonElement {
        base,
        label: label.into(),
    }
}
