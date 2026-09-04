use crate::{Lifecycle, SharedString};

use super::{
    node::{ElementBase, ElementKind, ElementNode},
    style::{Style, Styled},
    traits::Element,
};

/// Text element returned by [`text`].
#[derive(Clone, Debug)]
pub struct TextElement {
    base: ElementBase,
    value: SharedString,
}

impl Styled for TextElement {
    fn style(&mut self) -> &mut Style {
        &mut self.base.style
    }

    fn style_ref(&self) -> &Style {
        &self.base.style
    }
}

impl Element for TextElement {
    fn into_element_node(self) -> ElementNode {
        ElementNode::new(ElementKind::Text(self.value), self.base, Vec::new())
    }
}

impl Lifecycle for TextElement {}

#[must_use]
pub fn text(value: impl Into<SharedString>) -> TextElement {
    TextElement {
        base: ElementBase::default(),
        value: value.into(),
    }
}
