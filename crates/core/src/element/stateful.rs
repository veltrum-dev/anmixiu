use super::{
    node::ElementNode,
    style::{Style, Styled},
    traits::{Element, ParentElement},
};

/// Type-state wrapper produced after assigning an [`super::ElementId`].
#[derive(Clone, Debug)]
pub struct Stateful<E> {
    element: E,
}

impl<E> Stateful<E> {
    pub(crate) const fn new(element: E) -> Self {
        Self { element }
    }

    #[must_use]
    pub fn into_inner(self) -> E {
        self.element
    }

    #[must_use]
    pub const fn inner(&self) -> &E {
        &self.element
    }

    pub(crate) const fn inner_mut(&mut self) -> &mut E {
        &mut self.element
    }
}

impl<E: Styled> Styled for Stateful<E> {
    fn style(&mut self) -> &mut Style {
        self.element.style()
    }

    fn style_ref(&self) -> &Style {
        self.element.style_ref()
    }
}

impl<E: ParentElement> ParentElement for Stateful<E> {
    fn child_nodes(&mut self) -> &mut Vec<ElementNode> {
        self.element.child_nodes()
    }

    fn children_ref(&self) -> &[ElementNode] {
        self.element.children_ref()
    }
}

impl<E: Element> Element for Stateful<E> {
    fn into_element_node(self) -> ElementNode {
        self.element.into_element_node()
    }
}
