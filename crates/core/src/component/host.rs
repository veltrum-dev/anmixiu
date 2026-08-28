use std::rc::Rc;

use anmixiu_reactive::ReactiveStats;
use thiserror::Error;

use crate::{Element, ElementNode, IntoElement};

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
    mounted: bool,
    unmounted: bool,
}

impl<C: Render> ComponentHost<C> {
    #[must_use]
    pub fn new(component: Rc<C>, context: Context<C>) -> Self {
        Self {
            component,
            context,
            element: None,
            mounted: false,
            unmounted: false,
        }
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

impl<C: Render> Drop for ComponentHost<C> {
    fn drop(&mut self) {
        self.unmount();
    }
}
