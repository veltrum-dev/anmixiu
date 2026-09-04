use crate::{EventBindings, IntoElement, component::Context, element::EmptyElement};

/// Lifecycle contract shared by every public Element.
///
/// `on_mount` runs once after the Element's first successful paint, `render` runs inside this
/// Element's reactive observer, and `on_unmount` runs once when its mounted identity leaves the
/// tree. Ordinary elements inherit no-op mount/unmount hooks.
pub trait Lifecycle: Sized + 'static {
    fn bind_events(&self, _cx: &mut Context<Self>, _bindings: &mut EventBindings) {}

    fn on_mount(&self, _cx: &mut Context<Self>) {}

    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        EmptyElement::new()
    }

    fn on_unmount(&self, _cx: &mut Context<Self>) {}
}
