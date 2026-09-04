use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    fmt,
    rc::Rc,
};

use anmixiu_reactive::{OwnerId, ReactiveStats};
use thiserror::Error;

use crate::{
    Element, ElementNode, EventBindings, Eventful, GlobalElementId, IntoElement,
    component::ContextServices,
};

use super::{Context, Render, RenderOnce};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("component has already been unmounted")]
    Unmounted,
    #[error("component render did not produce an element")]
    MissingElement,
    #[error("nested Render component is missing its required semantic ElementId")]
    MissingComponentId,
    #[error("duplicate nested component identity `{0}` in one component render")]
    DuplicateComponentId(GlobalElementId),
}

pub(crate) trait NestedComponentFactory: fmt::Debug {
    fn component_type_id(&self) -> TypeId;
    fn instance_id(&self) -> *const ();
    fn mount(&self, services: &ContextServices) -> Box<dyn NestedComponentHost>;
}

pub(crate) trait NestedComponentHost {
    fn component_type_id(&self) -> TypeId;
    fn instance_id(&self) -> *const ();
    fn contains_owner(&self, owner: OwnerId) -> bool;
    fn render_snapshot(
        &mut self,
        dirty: &HashSet<OwnerId>,
        force: bool,
    ) -> Result<Rc<ElementNode>, RenderError>;
    fn did_paint(&mut self);
    fn unmount(&mut self);
}

struct TypedNestedFactory<C: Render> {
    component: Rc<C>,
    event_registrar: Option<fn(&C, &mut Context<C>, &mut EventBindings)>,
}

impl<C: Render> fmt::Debug for TypedNestedFactory<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NestedComponent")
            .field("component_type", &std::any::type_name::<C>())
            .finish_non_exhaustive()
    }
}

impl<C: Render> NestedComponentFactory for TypedNestedFactory<C> {
    fn component_type_id(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn instance_id(&self) -> *const () {
        Rc::as_ptr(&self.component).cast()
    }

    fn mount(&self, services: &ContextServices) -> Box<dyn NestedComponentHost> {
        let context = services.context::<C>();
        let host = match self.event_registrar {
            Some(registrar) => {
                ComponentHost::new_with_event_registrar(self.component.clone(), context, registrar)
            }
            None => ComponentHost::new(self.component.clone(), context),
        };
        Box::new(host)
    }
}

pub(crate) fn nested_component_factory<C: Render>(
    component: Rc<C>,
) -> Rc<dyn NestedComponentFactory> {
    Rc::new(TypedNestedFactory {
        component,
        event_registrar: None,
    })
}

pub(crate) fn nested_eventful_factory<C: Render + Eventful>(
    component: Rc<C>,
) -> Rc<dyn NestedComponentFactory> {
    Rc::new(TypedNestedFactory {
        component,
        event_registrar: Some(register_events::<C>),
    })
}

#[doc(hidden)]
pub struct ComponentHost<C: Render> {
    component: Rc<C>,
    context: Context<C>,
    template: Option<ElementNode>,
    element: Option<Rc<ElementNode>>,
    children: HashMap<GlobalElementId, Box<dyn NestedComponentHost>>,
    event_bindings: EventBindings,
    event_registrar: Option<fn(&C, &mut Context<C>, &mut EventBindings)>,
    mounted: bool,
    unmounted: bool,
}

impl<C: Render> ComponentHost<C> {
    #[must_use]
    pub fn new(component: Rc<C>, context: Context<C>) -> Self {
        let event_context = context.event_context();
        Self {
            component,
            context,
            template: None,
            element: None,
            children: HashMap::new(),
            event_bindings: EventBindings::new(event_context),
            event_registrar: None,
            mounted: false,
            unmounted: false,
        }
    }

    /// Creates a host that invokes the optional [`Eventful`] capability once at mount.
    #[must_use]
    pub fn new_eventful(component: Rc<C>, context: Context<C>) -> Self
    where
        C: Eventful,
    {
        Self::new_with_event_registrar(component, context, register_events::<C>)
    }

    /// Creates a host with an erased event-binding hook.
    ///
    /// This is used by platform adapters that select the optional [`Eventful`] capability without
    /// adding that capability as a bound on every [`Render`] value.
    #[doc(hidden)]
    #[must_use]
    pub fn new_with_event_registrar(
        component: Rc<C>,
        context: Context<C>,
        registrar: fn(&C, &mut Context<C>, &mut EventBindings),
    ) -> Self {
        let mut host = Self::new(component, context);
        host.event_registrar = Some(registrar);
        host
    }

    /// Renders the component inside its explicit reactive observer.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Unmounted`] after teardown, or `MissingElement` if the internal
    /// render contract cannot retain the returned element.
    pub fn render(&mut self) -> Result<&ElementNode, RenderError> {
        self.render_with(&HashSet::new(), true)?;
        self.element.as_deref().ok_or(RenderError::MissingElement)
    }

    /// Renders only owners present in `dirty`, preserving clean component snapshots.
    #[doc(hidden)]
    pub fn render_dirty(&mut self, dirty: &[OwnerId]) -> Result<&ElementNode, RenderError> {
        let dirty = dirty.iter().copied().collect();
        self.render_with(&dirty, false)?;
        self.element.as_deref().ok_or(RenderError::MissingElement)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn contains_owner(&self, owner: OwnerId) -> bool {
        self.context.owner == owner
            || self
                .children
                .values()
                .any(|child| child.contains_owner(owner))
    }

    fn render_with(&mut self, dirty: &HashSet<OwnerId>, force: bool) -> Result<(), RenderError> {
        if self.unmounted {
            return Err(RenderError::Unmounted);
        }
        if !force
            && self.element.is_some()
            && !dirty.iter().any(|owner| self.contains_owner(*owner))
        {
            return Ok(());
        }

        if force || self.template.is_none() || dirty.contains(&self.context.owner) {
            let registry = self.context.registry.clone();
            let owner = self.context.owner;
            let component = self.component.clone();
            let mut rendered = registry
                .observe(owner, || {
                    component
                        .render(&mut self.context)
                        .into_element()
                        .into_element_node()
                })
                .ok_or(RenderError::Unmounted)?;
            rendered.assign_owner(owner);
            self.template = Some(rendered);
        }

        let mut resolved = self.template.clone().ok_or(RenderError::MissingElement)?;
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        self.resolve_components(&mut resolved, dirty, &mut path, &mut seen)?;
        let removed = self
            .children
            .keys()
            .filter(|id| !seen.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut child) = self.children.remove(&id) {
                child.unmount();
            }
        }
        self.element = Some(Rc::new(resolved));
        Ok(())
    }

    fn resolve_components(
        &mut self,
        node: &mut ElementNode,
        dirty: &HashSet<OwnerId>,
        path: &mut Vec<crate::ElementId>,
        seen: &mut HashSet<GlobalElementId>,
    ) -> Result<(), RenderError> {
        let has_id = if let Some(id) = node.element_id_value().cloned() {
            path.push(id);
            true
        } else {
            false
        };

        if let Some(factory) = node.component_factory() {
            if !has_id {
                return Err(RenderError::MissingComponentId);
            }
            let id = GlobalElementId::new(path.iter().cloned());
            if !seen.insert(id.clone()) {
                return Err(RenderError::DuplicateComponentId(id));
            }
            let replace = self.children.get(&id).is_none_or(|child| {
                child.component_type_id() != factory.component_type_id()
                    || child.instance_id() != factory.instance_id()
            });
            if replace {
                if let Some(mut previous) = self.children.remove(&id) {
                    previous.unmount();
                }
                self.children
                    .insert(id.clone(), factory.mount(&self.context.services()));
            }
            let child = self
                .children
                .get_mut(&id)
                .ok_or(RenderError::MissingElement)?;
            let snapshot = child.render_snapshot(dirty, replace)?;
            node.set_component_child(snapshot.as_ref().clone());
        } else {
            for child in node.child_nodes_mut() {
                self.resolve_components(child, dirty, path, seen)?;
            }
        }

        if has_id {
            path.pop();
        }
        Ok(())
    }

    pub fn did_paint(&mut self) {
        if !self.unmounted && self.element.is_some() && !self.mounted {
            if let Some(register) = self.event_registrar.take() {
                register(&self.component, &mut self.context, &mut self.event_bindings);
            }
            self.component.on_mount(&mut self.context);
            self.mounted = true;
        }
        for child in self.children.values_mut() {
            child.did_paint();
        }
    }

    pub fn unmount(&mut self) {
        if self.unmounted {
            return;
        }
        for child in self.children.values_mut() {
            child.unmount();
        }
        self.children.clear();
        self.context.deactivate_owner();
        if self.mounted {
            self.component.on_unmount(&mut self.context);
        }
        let _removed = self.context.registry.remove_owner(self.context.owner);
        self.element = None;
        self.unmounted = true;
    }

    #[must_use]
    pub fn element(&self) -> Option<&ElementNode> {
        self.element.as_deref()
    }

    #[must_use]
    pub fn element_snapshot(&self) -> Option<Rc<ElementNode>> {
        self.element.clone()
    }

    #[must_use]
    pub fn reactive_stats(&self) -> ReactiveStats {
        self.context.registry.stats()
    }

    #[must_use]
    pub fn render_once<R: RenderOnce>(renderer: R, mut context: Context<R>) -> ElementNode {
        renderer
            .render(&mut context)
            .into_element()
            .into_element_node()
    }
}

impl<C: Render> NestedComponentHost for ComponentHost<C> {
    fn component_type_id(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn instance_id(&self) -> *const () {
        Rc::as_ptr(&self.component).cast()
    }

    fn contains_owner(&self, owner: OwnerId) -> bool {
        Self::contains_owner(self, owner)
    }

    fn render_snapshot(
        &mut self,
        dirty: &HashSet<OwnerId>,
        force: bool,
    ) -> Result<Rc<ElementNode>, RenderError> {
        self.render_with(dirty, force)?;
        self.element_snapshot().ok_or(RenderError::MissingElement)
    }

    fn did_paint(&mut self) {
        Self::did_paint(self);
    }

    fn unmount(&mut self) {
        Self::unmount(self);
    }
}

fn register_events<C: Render + Eventful>(
    component: &C,
    context: &mut Context<C>,
    bindings: &mut EventBindings,
) {
    component.bind_events(context, bindings);
}

impl<C: Render> Drop for ComponentHost<C> {
    fn drop(&mut self) {
        self.unmount();
    }
}
