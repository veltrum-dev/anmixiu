use std::rc::Rc;

use anmixiu_reactive::ReactiveStats;
use thiserror::Error;

use crate::{Element, ElementNode, EventBindings, Eventful, IntoElement};

use super::{Context, Render, RenderOnce};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("component has already been unmounted")]
    Unmounted,
    #[error("component render did not produce an element")]
    MissingElement,
}

pub struct ComponentHost<C: Render> {
    component: Rc<C>,
    context: Context<C>,
    element: Option<Rc<ElementNode>>,
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
            element: None,
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
        if self.unmounted {
            return Err(RenderError::Unmounted);
        }
        let registry = self.context.registry.clone();
        let owner = self.context.owner;
        let component = self.component.clone();
        let rendered = registry
            .observe(owner, || {
                component
                    .render(&mut self.context)
                    .into_element()
                    .into_element_node()
            })
            .ok_or(RenderError::Unmounted)?;
        self.element = Some(Rc::new(rendered));
        self.element.as_deref().ok_or(RenderError::MissingElement)
    }

    pub fn did_paint(&mut self) {
        if !self.unmounted && self.element.is_some() && !self.mounted {
            if let Some(register) = self.event_registrar.take() {
                register(&self.component, &mut self.context, &mut self.event_bindings);
            }
            self.component.on_mount(&mut self.context);
            self.mounted = true;
        }
    }

    pub fn unmount(&mut self) {
        if self.unmounted {
            return;
        }
        self.context.deactivate_owner();
        self.component.on_unmount(&mut self.context);
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
