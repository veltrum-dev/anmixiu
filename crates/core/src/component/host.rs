use std::{
    any::{Any, TypeId},
    collections::{HashMap, HashSet},
    fmt,
    rc::Rc,
};

use anmixiu_reactive::{OwnerId, ReactiveStats};
use thiserror::Error;

use crate::{Element, ElementNode, EventBindings, IntoElement, component::ContextServices};

use super::{Context, Lifecycle};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("element lifecycle has already been unmounted")]
    Unmounted,
    #[error("element lifecycle render did not produce an element")]
    MissingElement,
    #[error("duplicate mounted Element identity `{0}` in one lifecycle render")]
    DuplicateElementIdentity(String),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum MountedPathPart {
    Semantic(crate::ElementId),
    Position(usize),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct MountedPath(Vec<MountedPathPart>);

impl fmt::Display for MountedPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, part) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str("/")?;
            }
            match part {
                MountedPathPart::Semantic(id) => id.fmt(formatter)?,
                MountedPathPart::Position(position) => write!(formatter, "#{position}")?,
            }
        }
        Ok(())
    }
}

pub(crate) trait ElementLifecycleFactory: fmt::Debug {
    fn element_type_id(&self) -> TypeId;
    fn instance_id(&self) -> *const ();
    fn as_any(&self) -> &dyn Any;
    fn mount(&self, services: &ContextServices) -> Box<dyn MountedElementLifecycle>;
}

pub(crate) trait MountedElementLifecycle {
    fn element_type_id(&self) -> TypeId;
    fn instance_id(&self) -> *const ();
    fn update_from(&mut self, factory: &dyn ElementLifecycleFactory) -> bool;
    fn collect_owners(&self, owners: &mut HashSet<OwnerId>);
    fn render_snapshot(
        &mut self,
        dirty: &HashSet<OwnerId>,
        force: bool,
    ) -> Result<Rc<ElementNode>, RenderError>;
    fn did_paint(&mut self);
    fn unmount(&mut self);
}

struct TypedElementFactory<C: Element> {
    component: Rc<C>,
}

impl<C: Element> fmt::Debug for TypedElementFactory<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElementLifecycle")
            .field("element_type", &std::any::type_name::<C>())
            .finish_non_exhaustive()
    }
}

impl<C: Element> ElementLifecycleFactory for TypedElementFactory<C> {
    fn element_type_id(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn instance_id(&self) -> *const () {
        Rc::as_ptr(&self.component).cast()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn mount(&self, services: &ContextServices) -> Box<dyn MountedElementLifecycle> {
        let context = services.context::<C>();
        Box::new(ElementHost::new(self.component.clone(), context))
    }
}

pub(crate) fn element_lifecycle_factory<C: Element>(
    component: Rc<C>,
) -> Rc<dyn ElementLifecycleFactory> {
    Rc::new(TypedElementFactory { component })
}

#[doc(hidden)]
pub struct ElementHost<C: Lifecycle> {
    component: Rc<C>,
    context: Context<C>,
    template: Option<ElementNode>,
    element: Option<Rc<ElementNode>>,
    children: HashMap<MountedPath, Box<dyn MountedElementLifecycle>>,
    child_order: Vec<MountedPath>,
    owner_index: HashSet<OwnerId>,
    event_bindings: EventBindings,
    mounted: bool,
    unmounted: bool,
}

impl<C: Lifecycle> ElementHost<C> {
    #[must_use]
    pub fn new(component: Rc<C>, context: Context<C>) -> Self {
        let event_context = context.event_context();
        let owner_index = HashSet::from([context.owner]);
        Self {
            component,
            context,
            template: None,
            element: None,
            children: HashMap::new(),
            child_order: Vec::new(),
            owner_index,
            event_bindings: EventBindings::new(event_context),
            mounted: false,
            unmounted: false,
        }
    }

    /// Renders the Element inside its explicit reactive observer.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Unmounted`] after teardown, or `MissingElement` if the internal
    /// render contract cannot retain the returned element.
    pub fn render(&mut self) -> Result<&ElementNode, RenderError> {
        self.render_with(&HashSet::new(), true)?;
        self.element.as_deref().ok_or(RenderError::MissingElement)
    }

    /// Renders only owners present in `dirty`, preserving clean Element snapshots.
    #[doc(hidden)]
    pub fn render_dirty(&mut self, dirty: &[OwnerId]) -> Result<&ElementNode, RenderError> {
        let dirty = dirty.iter().copied().collect();
        self.render_with(&dirty, false)?;
        self.element.as_deref().ok_or(RenderError::MissingElement)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn contains_owner(&self, owner: OwnerId) -> bool {
        self.owner_index.contains(&owner)
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
                    Lifecycle::render(component.as_ref(), &mut self.context)
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
        let mut next_order = Vec::new();
        self.resolve_components(
            &mut resolved,
            dirty,
            &mut path,
            &mut seen,
            &mut next_order,
            0,
        )?;
        let removed = self
            .child_order
            .iter()
            .rev()
            .filter(|id| !seen.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut child) = self.children.remove(&id) {
                child.unmount();
            }
        }
        self.child_order = next_order;
        self.rebuild_owner_index();
        self.element = Some(Rc::new(resolved));
        Ok(())
    }

    fn rebuild_owner_index(&mut self) {
        let mut owners = HashSet::from([self.context.owner]);
        for id in &self.child_order {
            if let Some(child) = self.children.get(id) {
                child.collect_owners(&mut owners);
            }
        }
        self.owner_index = owners;
    }

    fn resolve_components(
        &mut self,
        node: &mut ElementNode,
        dirty: &HashSet<OwnerId>,
        path: &mut Vec<MountedPathPart>,
        seen: &mut HashSet<MountedPath>,
        order: &mut Vec<MountedPath>,
        position: usize,
    ) -> Result<(), RenderError> {
        if let Some(id) = node.element_id_value().cloned() {
            path.push(MountedPathPart::Semantic(id));
        } else {
            path.push(MountedPathPart::Position(position));
        }

        if let Some(factory) = node.lifecycle_factory() {
            let id = MountedPath(path.clone());
            if !seen.insert(id.clone()) {
                return Err(RenderError::DuplicateElementIdentity(id.to_string()));
            }
            order.push(id.clone());
            let replace = self
                .children
                .get(&id)
                .is_some_and(|child| child.element_type_id() != factory.element_type_id());
            let insert = !self.children.contains_key(&id) || replace;
            if insert {
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
            let configuration_changed = !insert
                && child.instance_id() != factory.instance_id()
                && child.update_from(factory.as_ref());
            let snapshot = child.render_snapshot(dirty, insert || configuration_changed)?;
            node.set_rendered_child(snapshot.as_ref().clone());
        } else {
            for (position, child) in node.child_nodes_mut().iter_mut().enumerate() {
                self.resolve_components(child, dirty, path, seen, order, position)?;
            }
        }

        path.pop();
        Ok(())
    }

    pub fn did_paint(&mut self) {
        if !self.unmounted && self.element.is_some() && !self.mounted {
            Lifecycle::bind_events(
                self.component.as_ref(),
                &mut self.context,
                &mut self.event_bindings,
            );
            Lifecycle::on_mount(self.component.as_ref(), &mut self.context);
            self.mounted = true;
        }
        for id in self.child_order.clone() {
            if let Some(child) = self.children.get_mut(&id) {
                child.did_paint();
            }
        }
    }

    pub fn unmount(&mut self) {
        if self.unmounted {
            return;
        }
        for id in self.child_order.iter().rev() {
            if let Some(child) = self.children.get_mut(id) {
                child.unmount();
            }
        }
        self.children.clear();
        self.child_order.clear();
        self.context.deactivate_owner();
        if self.mounted {
            Lifecycle::on_unmount(self.component.as_ref(), &mut self.context);
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
}

impl<C: Element> MountedElementLifecycle for ElementHost<C> {
    fn element_type_id(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn instance_id(&self) -> *const () {
        Rc::as_ptr(&self.component).cast()
    }

    fn update_from(&mut self, factory: &dyn ElementLifecycleFactory) -> bool {
        let Some(factory) = factory.as_any().downcast_ref::<TypedElementFactory<C>>() else {
            return false;
        };
        self.component = factory.component.clone();
        true
    }

    fn collect_owners(&self, owners: &mut HashSet<OwnerId>) {
        owners.extend(self.owner_index.iter().copied());
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

impl<C: Lifecycle> Drop for ElementHost<C> {
    fn drop(&mut self) {
        self.unmount();
    }
}
