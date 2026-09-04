use std::rc::Rc;

use crate::{
    Eventful, Render,
    component::{NestedComponentFactory, nested_component_factory, nested_eventful_factory},
};

use super::{
    id::ElementId,
    node::{ElementBase, ElementKind, ElementNode},
    traits::Element,
};

/// A persistent nested component boundary.
///
/// Keep the supplied [`Rc`] stable across parent renders and assign a semantic id with
/// [`Self::id`]. The id and component type identify the retained lifecycle slot.
#[derive(Clone, Debug)]
pub struct ComponentElement {
    base: ElementBase,
    factory: Rc<dyn NestedComponentFactory>,
}

impl ComponentElement {
    /// Assigns the semantic identity used to retain this component's lifecycle slot.
    #[must_use]
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.base.id = Some(id.into());
        self
    }
}

impl Element for ComponentElement {
    fn into_element_node(self) -> ElementNode {
        ElementNode::new(ElementKind::Component(self.factory), self.base, Vec::new())
    }
}

/// Creates a persistent nested [`Render`] component boundary.
#[must_use]
pub fn component<C: Render>(component: Rc<C>) -> ComponentElement {
    ComponentElement {
        base: ElementBase::default(),
        factory: nested_component_factory(component),
    }
}

/// Creates a persistent nested component whose [`Eventful`] bindings follow its lifecycle.
#[must_use]
pub fn eventful_component<C: Render + Eventful>(component: Rc<C>) -> ComponentElement {
    ComponentElement {
        base: ElementBase::default(),
        factory: nested_eventful_factory(component),
    }
}
