use crate::{EventBindings, IntoElement, component::Context};

/// Optional event capability for a persistent Element.
///
/// A host explicitly created as eventful invokes [`bind_events`](Self::bind_events) once when the
/// Element is mounted. At the application boundary this means using `App::run_eventful`; ordinary
/// `App::run` deliberately does not inspect optional traits. The registration set owns the returned
/// subscriptions and drops them with the Element owner, so ordinary Elements pay no event lifecycle
/// cost unless their host opts in to this capability.
pub trait Eventful: Sized + 'static {
    fn bind_events(&self, _cx: &mut Context<Self>, _bindings: &mut EventBindings) {}
}

/// Persistent component contract.
///
/// Components own signals and lifecycle; custom element values implement [`crate::Element`].
pub trait Render: Sized + 'static {
    fn on_mount(&self, _cx: &mut Context<Self>) {}

    fn render(&self, cx: &mut Context<Self>) -> impl IntoElement;

    fn on_unmount(&self, _cx: &mut Context<Self>) {}
}

/// Consuming component recipe with no persistent lifecycle.
pub trait RenderOnce: Sized + 'static {
    fn render(self, cx: &mut Context<Self>) -> impl IntoElement;
}
