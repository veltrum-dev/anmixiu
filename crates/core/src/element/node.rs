use std::rc::Rc;

use crate::{SharedString, component::NestedComponentFactory};

use super::{
    id::ElementId,
    interaction::{ClickHandler, HoverHandler},
    style::{Style, StyleRefinement},
    traits::Element,
};

#[derive(Clone, Debug)]
pub(crate) enum ElementKind {
    Div,
    Text(SharedString),
    Button(SharedString),
    Component(Rc<dyn NestedComponentFactory>),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ElementBase {
    pub style: Style,
    pub id: Option<ElementId>,
    pub click: Option<ClickHandler>,
    pub hover_handler: Option<HoverHandler>,
    pub hover: Option<StyleRefinement>,
    pub scroll: Option<super::scroll::ScrollHandle>,
}

/// Type-erased node shared by the core, layout, and platform crates.
///
/// Public builders return concrete element types; application code should not construct this
/// projection directly.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct ElementNode {
    kind: ElementKind,
    base: ElementBase,
    children: Vec<Self>,
}

impl ElementNode {
    pub(crate) const fn new(kind: ElementKind, base: ElementBase, children: Vec<Self>) -> Self {
        Self {
            kind,
            base,
            children,
        }
    }

    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self.kind {
            ElementKind::Div => "div",
            ElementKind::Text(_) => "text",
            ElementKind::Button(_) => "button",
            ElementKind::Component(_) => "component",
        }
    }

    #[must_use]
    pub fn text_content(&self) -> Option<&str> {
        match &self.kind {
            ElementKind::Text(value) | ElementKind::Button(value) => Some(value),
            ElementKind::Div | ElementKind::Component(_) => None,
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn text_value(&self) -> Option<&SharedString> {
        match &self.kind {
            ElementKind::Text(value) | ElementKind::Button(value) => Some(value),
            ElementKind::Div | ElementKind::Component(_) => None,
        }
    }

    /// The scroll handle if this element is a scroll container, else `None`.
    #[doc(hidden)]
    #[must_use]
    pub fn scroll_handle(&self) -> Option<&super::scroll::ScrollHandle> {
        self.base.scroll.as_ref()
    }

    pub(crate) fn assign_owner(&mut self, owner: anmixiu_reactive::OwnerId) {
        if let Some(handler) = self.base.click.as_mut() {
            handler.bind_owner(owner);
        }
        if !matches!(self.kind, ElementKind::Component(_)) {
            for child in &mut self.children {
                child.assign_owner(owner);
            }
        }
    }

    pub(crate) fn element_id_value(&self) -> Option<&ElementId> {
        self.base.id.as_ref()
    }

    pub(crate) fn component_factory(&self) -> Option<Rc<dyn NestedComponentFactory>> {
        match &self.kind {
            ElementKind::Component(factory) => Some(factory.clone()),
            ElementKind::Div | ElementKind::Text(_) | ElementKind::Button(_) => None,
        }
    }

    pub(crate) fn set_component_child(&mut self, child: Self) {
        debug_assert!(matches!(self.kind, ElementKind::Component(_)));
        self.children.clear();
        self.children.push(child);
    }

    pub(crate) fn child_nodes_mut(&mut self) -> &mut [Self] {
        &mut self.children
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn style_ref(&self) -> &Style {
        &self.base.style
    }

    #[doc(hidden)]
    #[must_use]
    pub fn children_ref(&self) -> &[Self] {
        &self.children
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn element_id(&self) -> Option<&ElementId> {
        self.base.id.as_ref()
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn click_handler(&self) -> Option<&ClickHandler> {
        self.base.click.as_ref()
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn hover_style(&self) -> Option<&StyleRefinement> {
        self.base.hover.as_ref()
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn hover_handler(&self) -> Option<&HoverHandler> {
        self.base.hover_handler.as_ref()
    }
}

impl Element for ElementNode {
    fn into_element_node(self) -> ElementNode {
        self
    }
}
