use std::{cell::Cell, rc::Rc};

use anmixiu_core::{
    Context, DivElement, Element, ElementHost, ElementNode, IntoElement, Lifecycle, ParentElement,
    Pixels, Style, Styled, div, text,
};
use anmixiu_reactive::Signal;

struct UserInfoElement {
    root: DivElement,
    mount_event: &'static str,
    unmount_event: &'static str,
    label: Signal<&'static str>,
    renders: Rc<Cell<usize>>,
    events: Rc<std::cell::RefCell<Vec<&'static str>>>,
}

impl UserInfoElement {
    fn new(
        mount_event: &'static str,
        unmount_event: &'static str,
        label: Signal<&'static str>,
        renders: Rc<Cell<usize>>,
        events: Rc<std::cell::RefCell<Vec<&'static str>>>,
    ) -> Self {
        Self {
            root: div(),
            mount_event,
            unmount_event,
            label,
            renders,
            events,
        }
    }
}

impl Styled for UserInfoElement {
    fn style(&mut self) -> &mut Style {
        self.root.style()
    }

    fn style_ref(&self) -> &Style {
        self.root.style_ref()
    }
}

impl Lifecycle for UserInfoElement {
    fn on_mount(&self, _cx: &mut Context<Self>) {
        self.events.borrow_mut().push(self.mount_event);
    }

    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        text(self.label.get())
    }

    fn on_unmount(&self, _cx: &mut Context<Self>) {
        self.events.borrow_mut().push(self.unmount_event);
    }
}

impl Element for UserInfoElement {}

struct UserElement {
    root: DivElement,
    renders: Rc<Cell<usize>>,
    events: Rc<std::cell::RefCell<Vec<&'static str>>>,
}

impl UserElement {
    fn new(renders: Rc<Cell<usize>>, events: Rc<std::cell::RefCell<Vec<&'static str>>>) -> Self {
        Self {
            root: div(),
            renders,
            events,
        }
    }
}

impl Styled for UserElement {
    fn style(&mut self) -> &mut Style {
        self.root.style()
    }

    fn style_ref(&self) -> &Style {
        self.root.style_ref()
    }
}

impl ParentElement for UserElement {
    fn child_nodes(&mut self) -> &mut Vec<ElementNode> {
        self.root.child_nodes()
    }

    fn children_ref(&self) -> &[ElementNode] {
        self.root.children_ref()
    }
}

impl Lifecycle for UserElement {
    fn on_mount(&self, _cx: &mut Context<Self>) {
        self.events.borrow_mut().push("user mount");
    }

    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        self.root.clone()
    }

    fn on_unmount(&self, _cx: &mut Context<Self>) {
        self.events.borrow_mut().push("user unmount");
    }
}

impl Element for UserElement {}

#[test]
fn child_elements_mount_once_update_precisely_and_unmount_in_reverse_order() {
    let events = Rc::new(std::cell::RefCell::new(Vec::new()));
    let user_renders = Rc::new(Cell::new(0));
    let first_info_renders = Rc::new(Cell::new(0));
    let second_info_renders = Rc::new(Cell::new(0));
    let label = Signal::new("Alice");
    let root = UserElement::new(user_renders.clone(), events.clone())
        .child(
            UserInfoElement::new(
                "first info mount",
                "first info unmount",
                label.clone(),
                first_info_renders.clone(),
                events.clone(),
            )
            .width(77.0),
        )
        .child(UserInfoElement::new(
            "second info mount",
            "second info unmount",
            Signal::new("Static"),
            second_info_renders.clone(),
            events.clone(),
        ));
    let context = Context::testing();
    let owners = context.owner_registry().clone();
    let mut host = ElementHost::new(Rc::new(root), context);

    host.render().expect("initial lifecycle tree renders");
    host.did_paint();
    assert_eq!(
        &*events.borrow(),
        &["user mount", "first info mount", "second info mount"]
    );
    assert_eq!(user_renders.get(), 1);
    assert_eq!(first_info_renders.get(), 1);
    assert_eq!(second_info_renders.get(), 1);
    let first_info = host
        .element()
        .and_then(|root| root.children_ref().first())
        .expect("first custom child has its own retained box");
    assert_eq!(first_info.kind_name(), "element");
    assert_eq!(first_info.style_ref().width.map(Pixels::value), Some(77.0));

    label.set("Bob");
    let dirty = owners.take_dirty();
    assert_eq!(dirty.len(), 1, "Signal routes directly to UserInfoElement");
    host.render_dirty(&dirty).expect("only dirty child renders");
    host.did_paint();
    assert_eq!(user_renders.get(), 1, "clean parent render is retained");
    assert_eq!(first_info_renders.get(), 2);
    assert_eq!(
        second_info_renders.get(),
        1,
        "clean sibling render is retained"
    );
    assert_eq!(
        &*events.borrow(),
        &["user mount", "first info mount", "second info mount"],
        "an update never mounts again"
    );

    host.unmount();
    assert_eq!(
        &*events.borrow(),
        &[
            "user mount",
            "first info mount",
            "second info mount",
            "second info unmount",
            "first info unmount",
            "user unmount"
        ]
    );
}

struct PlainLabelElement {
    style: Style,
    value: &'static str,
    mounts: Rc<Cell<usize>>,
    renders: Rc<Cell<usize>>,
}

impl Styled for PlainLabelElement {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }

    fn style_ref(&self) -> &Style {
        &self.style
    }
}

impl Lifecycle for PlainLabelElement {
    fn on_mount(&self, _cx: &mut Context<Self>) {
        self.mounts.set(self.mounts.get() + 1);
    }

    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        text(self.value)
    }
}

impl Element for PlainLabelElement {}

struct ConfigParent {
    value: Signal<&'static str>,
    mounts: Rc<Cell<usize>>,
    renders: Rc<Cell<usize>>,
}

impl Lifecycle for ConfigParent {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(PlainLabelElement {
            style: Style::default(),
            value: self.value.get(),
            mounts: self.mounts.clone(),
            renders: self.renders.clone(),
        })
    }
}

#[test]
fn same_identity_receives_new_configuration_without_remounting() {
    let value = Signal::new("Alice");
    let mounts = Rc::new(Cell::new(0));
    let renders = Rc::new(Cell::new(0));
    let context = Context::testing();
    let owners = context.owner_registry().clone();
    let mut host = ElementHost::new(
        Rc::new(ConfigParent {
            value: value.clone(),
            mounts: mounts.clone(),
            renders: renders.clone(),
        }),
        context,
    );

    host.render().expect("initial configuration renders");
    host.did_paint();
    assert_eq!(mounts.get(), 1);
    assert_eq!(renders.get(), 1);

    value.set("Bob");
    let dirty = owners.take_dirty();
    host.render_dirty(&dirty).expect("configuration updates");
    host.did_paint();
    assert_eq!(mounts.get(), 1, "same identity never mounts twice");
    assert_eq!(renders.get(), 2);
}
