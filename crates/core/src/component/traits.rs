use crate::{IntoElement, component::Context};

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
