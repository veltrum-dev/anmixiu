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

struct ReconcileState<'a> {
    dirty: &'a HashSet<OwnerId>,
    mounted_path: Vec<MountedPathPart>,
    node_location: Vec<usize>,
    seen: HashSet<MountedPath>,
    next_order: Vec<MountedPath>,
    next_locations: HashMap<MountedPath, Vec<usize>>,
}

impl<'a> ReconcileState<'a> {
    fn new(dirty: &'a HashSet<OwnerId>) -> Self {
        Self {
            dirty,
            mounted_path: Vec::new(),
            node_location: Vec::new(),
            seen: HashSet::new(),
            next_order: Vec::new(),
            next_locations: HashMap::new(),
        }
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
    // Key: a direct mounted child's semantic/positional path. Rebuilt whenever this owner's
    // template changes; bounded by the number of lifecycle children in that template.
    child_locations: HashMap<MountedPath, Vec<usize>>,
    // Key: the same direct child path. Refreshed after that child's subtree changes; bounded by
    // the number of live descendant owners.
    child_owners: HashMap<MountedPath, HashSet<OwnerId>>,
    // Key: a live descendant owner. Refreshed with `child_owners`; bounded by live owners and used
    // to select the one direct branch that can contain a dirty owner.
    owner_routes: HashMap<OwnerId, MountedPath>,
    // Membership projection of this owner plus `owner_routes`, with the same invalidation and
    // capacity bound. Native window drivers use it to reject unrelated dirty owners in O(1).
    owner_index: HashSet<OwnerId>,
    event_bindings: EventBindings,
    mounted: bool,
    unmounted: bool,
    #[cfg(test)]
    last_update_visit_count: usize,
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
            child_locations: HashMap::new(),
            child_owners: HashMap::new(),
            owner_routes: HashMap::new(),
            owner_index,
            event_bindings: EventBindings::new(event_context),
            mounted: false,
            unmounted: false,
            #[cfg(test)]
            last_update_visit_count: 0,
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
        #[cfg(test)]
        {
            self.last_update_visit_count = 0;
        }
        if self.unmounted {
            return Err(RenderError::Unmounted);
        }
        if !force
            && self.element.is_some()
            && !dirty.iter().any(|owner| self.contains_owner(*owner))
        {
            return Ok(());
        }

        let rebuild_template =
            force || self.template.is_none() || dirty.contains(&self.context.owner);
        if rebuild_template {
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

        if !rebuild_template && self.element.is_some() {
            return self.render_dirty_children(dirty);
        }

        let mut resolved = self.template.clone().ok_or(RenderError::MissingElement)?;
        let mut reconcile = ReconcileState::new(dirty);
        self.resolve_components(&mut resolved, &mut reconcile, 0)?;
        let removed = self
            .child_order
            .iter()
            .rev()
            .filter(|id| !reconcile.seen.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed {
            if let Some(mut child) = self.children.remove(&id) {
                child.unmount();
            }
        }
        self.child_order = reconcile.next_order;
        self.child_locations = reconcile.next_locations;
        self.rebuild_owner_index();
        self.element = Some(Rc::new(resolved));
        Ok(())
    }

    fn render_dirty_children(&mut self, dirty: &HashSet<OwnerId>) -> Result<(), RenderError> {
        let mut seen = HashSet::new();
        let affected = dirty
            .iter()
            .filter_map(|owner| self.owner_routes.get(owner))
            .filter(|path| seen.insert((*path).clone()))
            .cloned()
            .collect::<Vec<_>>();

        for id in affected {
            #[cfg(test)]
            {
                self.last_update_visit_count += 1;
            }
            let snapshot = self
                .children
                .get_mut(&id)
                .ok_or(RenderError::MissingElement)?
                .render_snapshot(dirty, false)?;
            let location = self
                .child_locations
                .get(&id)
                .ok_or(RenderError::MissingElement)?;
            let root = Rc::make_mut(self.element.as_mut().ok_or(RenderError::MissingElement)?);
            let node = node_at_location_mut(root, location).ok_or(RenderError::MissingElement)?;
            node.set_rendered_child(snapshot.as_ref().clone());
            self.refresh_child_owners(&id)?;
        }
        Ok(())
    }

    fn rebuild_owner_index(&mut self) {
        self.owner_index.clear();
        self.owner_index.insert(self.context.owner);
        self.owner_routes.clear();
        self.child_owners.clear();
        for id in &self.child_order {
            if let Some(child) = self.children.get(id) {
                let mut owners = HashSet::new();
                child.collect_owners(&mut owners);
                for owner in &owners {
                    self.owner_routes.insert(*owner, id.clone());
                }
                self.owner_index.extend(owners.iter().copied());
                self.child_owners.insert(id.clone(), owners);
            }
        }
    }

    fn refresh_child_owners(&mut self, id: &MountedPath) -> Result<(), RenderError> {
        if let Some(previous) = self.child_owners.remove(id) {
            for owner in previous {
                self.owner_routes.remove(&owner);
                self.owner_index.remove(&owner);
            }
        }
        let child = self.children.get(id).ok_or(RenderError::MissingElement)?;
        let mut owners = HashSet::new();
        child.collect_owners(&mut owners);
        for owner in &owners {
            self.owner_routes.insert(*owner, id.clone());
        }
        self.owner_index.extend(owners.iter().copied());
        self.child_owners.insert(id.clone(), owners);
        Ok(())
    }

    fn resolve_components(
        &mut self,
        node: &mut ElementNode,
        reconcile: &mut ReconcileState<'_>,
        position: usize,
    ) -> Result<(), RenderError> {
        if let Some(id) = node.element_id_value().cloned() {
            reconcile.mounted_path.push(MountedPathPart::Semantic(id));
        } else {
            reconcile
                .mounted_path
                .push(MountedPathPart::Position(position));
        }

        if let Some(factory) = node.lifecycle_factory() {
            #[cfg(test)]
            {
                self.last_update_visit_count += 1;
            }
            let id = MountedPath(reconcile.mounted_path.clone());
            if !reconcile.seen.insert(id.clone()) {
                return Err(RenderError::DuplicateElementIdentity(id.to_string()));
            }
            reconcile.next_order.push(id.clone());
            reconcile
                .next_locations
                .insert(id.clone(), reconcile.node_location.clone());
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
            let snapshot =
                child.render_snapshot(reconcile.dirty, insert || configuration_changed)?;
            node.set_rendered_child(snapshot.as_ref().clone());
        } else {
            for (position, child) in node.child_nodes_mut().iter_mut().enumerate() {
                reconcile.node_location.push(position);
                self.resolve_components(child, reconcile, position)?;
                reconcile.node_location.pop();
            }
        }

        reconcile.mounted_path.pop();
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
        self.child_locations.clear();
        self.child_owners.clear();
        self.owner_routes.clear();
        self.owner_index.clear();
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

fn node_at_location_mut<'a>(
    mut node: &'a mut ElementNode,
    location: &[usize],
) -> Option<&'a mut ElementNode> {
    for &position in location {
        node = node.child_nodes_mut().get_mut(position)?;
    }
    Some(node)
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

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use anmixiu_reactive::Signal;

    use crate::{
        Context, Element, IntoElement, Lifecycle, ParentElement, Style, Styled, div, text,
    };

    use super::ElementHost;

    struct Label {
        style: Style,
        value: Signal<u32>,
    }

    impl Styled for Label {
        fn style(&mut self) -> &mut Style {
            &mut self.style
        }

        fn style_ref(&self) -> &Style {
            &self.style
        }
    }

    impl Lifecycle for Label {
        fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
            text(self.value.get().to_string())
        }
    }

    impl Element for Label {}

    struct Labels {
        values: Vec<Signal<u32>>,
    }

    impl Lifecycle for Labels {
        fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
            self.values.iter().cloned().fold(div(), |root, value| {
                root.child(Label {
                    style: Style::default(),
                    value,
                })
            })
        }
    }

    #[test]
    fn dirty_leaf_visits_only_its_direct_lifecycle_branch() {
        let values = (0..32).map(|_| Signal::new(0)).collect::<Vec<_>>();
        let target = values.last().cloned().expect("a target Signal");
        let context = Context::testing();
        let owners = context.owner_registry().clone();
        let mut host = ElementHost::new(Rc::new(Labels { values }), context);
        host.render().expect("initial tree");

        target.set(1);
        host.render_dirty(&owners.take_dirty()).expect("dirty leaf");

        assert_eq!(host.last_update_visit_count, 1);
    }
}
