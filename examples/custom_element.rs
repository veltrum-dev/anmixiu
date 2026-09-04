#![forbid(unsafe_code)]

use anmixiu::prelude::*;

#[derive(Element)]
struct StatusBadge {
    #[element(style)]
    root: DivElement,
    label: SharedString,
}

impl StatusBadge {
    fn new(label: impl Into<SharedString>) -> Self {
        Self {
            root: div()
                .padding(8.0)
                .background(Color::rgb(0.12, 0.22, 0.32))
                .foreground(Color::rgb(0.62, 0.84, 1.0))
                .rounded(8.0),
            label: label.into(),
        }
    }
}

impl Lifecycle for StatusBadge {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        text(self.label.clone())
    }
}

#[derive(Element)]
struct CustomPanel {
    #[element(style, parent)]
    root: DivElement,
    clicks: Signal<u32>,
}

impl CustomPanel {
    fn new() -> Self {
        Self {
            root: div()
                .width(520.0)
                .padding(28.0)
                .gap(16.0)
                .background(Color::rgb(0.035, 0.05, 0.09))
                .foreground(Color::WHITE)
                .rounded(18.0),
            clicks: Signal::new(0),
        }
    }
}

impl Lifecycle for CustomPanel {
    fn on_mount(&self, _cx: &mut Context<Self>) {
        eprintln!("CustomPanel mounted");
    }

    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.get();
        let increment = self.clicks.clone();
        self.root
            .clone()
            .child(text("Custom Element example").foreground(Color::rgb(0.65, 0.8, 1.0)))
            .child(text("The panel and badge are both custom Elements."))
            .child(text(shared_format!("Clicks: {clicks}")))
            .child(
                button("Increment Signal")
                    .height(44.0)
                    .id("increment-custom-element")
                    .on_click(move || increment.update(|value| *value += 1)),
            )
    }

    fn on_unmount(&self, _cx: &mut Context<Self>) {
        eprintln!("CustomPanel unmounted");
    }
}

fn main() -> Result<(), anmixiu::AppError> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let panel = CustomPanel::new().child(StatusBadge::new("Mounted with #[derive(Element)]"));
    let view = div()
        .padding(32.0)
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .background(Color::rgb(0.012, 0.018, 0.035))
        .child(panel);
    App::new()
        .window(
            Window::new()
                .title("Anmixiu Custom Element")
                .size(700.0, 500.0),
        )
        .run(view)
}
