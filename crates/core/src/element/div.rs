use super::{
    id::ElementId,
    interaction::{ClickHandler, HoverHandler, InteractiveElement},
    node::{ElementBase, ElementKind, ElementNode},
    style::{Style, StyleRefinement, Styled},
    traits::{Element, ParentElement},
};

/// Flex container element returned by [`div`].
#[derive(Clone, Debug, Default)]
pub struct DivElement {
    base: ElementBase,
    children: Vec<ElementNode>,
}

impl Styled for DivElement {
    fn style(&mut self) -> &mut Style {
        &mut self.base.style
    }

    fn style_ref(&self) -> &Style {
        &self.base.style
    }
}

impl ParentElement for DivElement {
    fn child_nodes(&mut self) -> &mut Vec<ElementNode> {
        &mut self.children
    }

    fn children_ref(&self) -> &[ElementNode] {
        &self.children
    }
}

impl InteractiveElement for DivElement {
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

impl DivElement {
    /// Makes this div a two-dimensional scroll container driven by `handle`.
    ///
    /// Content larger than the div's own bounds is clipped to the div's viewport and shifted by the
    /// handle's offsets; a wheel gesture over the div updates the handle. Give the div bounded
    /// dimensions (e.g. [`width`](Styled::width), [`height`](Styled::height), or a flex context) so
    /// there is a viewport to scroll within.
    #[must_use]
    pub fn scroll(mut self, handle: &crate::ScrollHandle) -> Self {
        handle.track_paint();
        self.base.scroll = Some(handle.clone());
        self
    }

    /// Alias for [`scroll`](Self::scroll) that makes the two-dimensional behavior explicit.
    #[must_use]
    pub fn scrollable(self, handle: &crate::ScrollHandle) -> Self {
        self.scroll(handle)
    }
}

impl Element for DivElement {
    fn into_element_node(self) -> ElementNode {
        ElementNode::new(ElementKind::Div, self.base, self.children)
    }
}

#[must_use]
pub fn div() -> DivElement {
    DivElement::default()
}
