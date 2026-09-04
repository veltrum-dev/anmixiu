#![cfg(feature = "macros")]

use anmixiu::prelude::*;

#[derive(Element)]
struct MacroParent {
    #[element(style, parent)]
    root: DivElement,
}

impl MacroParent {
    fn new() -> Self {
        Self { root: div() }
    }
}

impl Lifecycle for MacroParent {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        self.root.clone()
    }
}

#[derive(Element)]
struct MacroLeaf {
    #[element(style)]
    style: Style,
}

impl Lifecycle for MacroLeaf {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        text("leaf")
    }
}

#[test]
fn derive_element_delegates_style_and_parent_capabilities() {
    let parent = MacroParent::new().width(240.0).child(MacroLeaf {
        style: Style::default(),
    });

    assert_eq!(parent.root.style_ref().width, Some(px(240.0)));
    assert_eq!(parent.root.children_ref().len(), 1);
}
