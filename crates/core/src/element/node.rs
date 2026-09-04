use std::rc::Rc;

use crate::{
    Lifecycle, SharedString,
    component::{ElementLifecycleFactory, element_lifecycle_factory},
};

use super::{
    id::ElementId,
    interaction::{ClickHandler, HoverHandler},
    style::{Style, StyleRefinement, Styled},
    traits::Element,
};

#[derive(Clone, Debug)]
pub(crate) enum ElementKind {
    Empty,
    Div,
    Text(SharedString),
    Button(SharedString),
    Lifecycle(Rc<dyn ElementLifecycleFactory>),
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

    pub(crate) fn lifecycle<E: super::Element>(element: E) -> Self {
        let mut base = ElementBase {
            style: element.style_ref().clone(),
            ..ElementBase::default()
        };
        let factory = element_lifecycle_factory(std::rc::Rc::new(element));
        base.id = None;
        Self::new(ElementKind::Lifecycle(factory), base, Vec::new())
    }

    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self.kind {
            ElementKind::Empty => "empty",
            ElementKind::Div => "div",
            ElementKind::Text(_) => "text",
            ElementKind::Button(_) => "button",
            ElementKind::Lifecycle(_) => "element",
        }
    }

    #[must_use]
    pub fn text_content(&self) -> Option<&str> {
        match &self.kind {
            ElementKind::Text(value) | ElementKind::Button(value) => Some(value),
            ElementKind::Empty | ElementKind::Div | ElementKind::Lifecycle(_) => None,
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn text_value(&self) -> Option<&SharedString> {
        match &self.kind {
            ElementKind::Text(value) | ElementKind::Button(value) => Some(value),
            ElementKind::Empty | ElementKind::Div | ElementKind::Lifecycle(_) => None,
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
        if !matches!(self.kind, ElementKind::Lifecycle(_)) {
            for child in &mut self.children {
                child.assign_owner(owner);
            }
        }
    }

    pub(crate) fn element_id_value(&self) -> Option<&ElementId> {
        self.base.id.as_ref()
    }

    pub(crate) fn set_element_id(&mut self, id: ElementId) {
        self.base.id = Some(id);
    }

    pub(crate) fn lifecycle_factory(&self) -> Option<Rc<dyn ElementLifecycleFactory>> {
        match &self.kind {
            ElementKind::Lifecycle(factory) => Some(factory.clone()),
            ElementKind::Empty
            | ElementKind::Div
            | ElementKind::Text(_)
            | ElementKind::Button(_) => None,
        }
    }

    pub(crate) fn set_rendered_child(&mut self, child: Self) {
        debug_assert!(matches!(self.kind, ElementKind::Lifecycle(_)));
        self.children.clear();
        if !matches!(child.kind, ElementKind::Empty) {
            self.children.push(child);
        }
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

impl Lifecycle for ElementNode {}

impl Styled for ElementNode {
    fn style(&mut self) -> &mut Style {
        &mut self.base.style
    }

    fn style_ref(&self) -> &Style {
        &self.base.style
    }
}
