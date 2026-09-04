use crate::Lifecycle;

use super::{
    node::{ElementBase, ElementKind, ElementNode},
    style::{Style, Styled},
    traits::Element,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct EmptyElement {
    style: Style,
}

impl EmptyElement {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl Styled for EmptyElement {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }

    fn style_ref(&self) -> &Style {
        &self.style
    }
}

impl Lifecycle for EmptyElement {}

impl Element for EmptyElement {
    fn into_element_node(self) -> ElementNode {
        ElementNode::new(ElementKind::Empty, ElementBase::default(), Vec::new())
    }
}
