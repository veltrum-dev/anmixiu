use super::node::ElementNode;

/// A value that participates in an element tree.
///
/// Custom elements implement this trait. Persistent stateful components instead implement
/// [`crate::Render`] and produce an element tree from their signals.
pub trait Element: Sized + 'static {
    fn into_element_node(self) -> ElementNode;
}

/// Conversion contract used by component render methods and parent builders.
pub trait IntoElement: Sized {
    type Element: Element;

    fn into_element(self) -> Self::Element;
}

impl<E: Element> IntoElement for E {
    type Element = E;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Parenting capability, separate from styling and interaction.
pub trait ParentElement: Sized {
    #[doc(hidden)]
    fn child_nodes(&mut self) -> &mut Vec<ElementNode>;

    #[doc(hidden)]
    fn children_ref(&self) -> &[ElementNode];

    #[must_use]
    fn child(mut self, child: impl IntoElement) -> Self {
        self.child_nodes()
            .push(child.into_element().into_element_node());
        self
    }

    #[must_use]
    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.child_nodes().extend(
            children
                .into_iter()
                .map(|child| child.into_element().into_element_node()),
        );
        self
    }
}
