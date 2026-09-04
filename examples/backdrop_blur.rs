#![forbid(unsafe_code)]

use anmixiu::prelude::*;

struct BlurShowcase {
    style: Style,
    enabled: Signal<bool>,
}

impl Default for BlurShowcase {
    fn default() -> Self {
        Self {
            style: Style::default(),
            enabled: Signal::new(true),
        }
    }
}

impl Styled for BlurShowcase {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }
    fn style_ref(&self) -> &Style {
        &self.style
    }
}

impl Lifecycle for BlurShowcase {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.enabled.get();
        let toggle = self.enabled.clone();
        let panel = div()
            .w(520.0)
            .h(320.0)
            .p(44.0)
            .gap(18.0)
            .bg(Color::rgba(0.06, 0.08, 0.14, 0.52))
            .text_color(Color::WHITE)
            .rounded(36.0)
            .when(enabled, |panel| panel.backdrop_blur(20.0))
            .child(text("Anmixiu Backdrop Blur").text_color(Color::rgb(0.95, 0.98, 1.0)))
            .child(
                text(if enabled {
                    "Blur ON · the colored backdrop is filtered"
                } else {
                    "Blur OFF · the colored edge stays sharp"
                })
                .text_color(Color::rgb(0.72, 0.82, 0.96)),
            )
            .child(
                text("The panel fill, text, and button are painted afterward and remain sharp.")
                    .text_color(Color::rgb(0.78, 0.82, 0.9)),
            )
            .child(
                button(if enabled {
                    "Disable backdrop blur"
                } else {
                    "Enable backdrop blur"
                })
                .h(48.0)
                .id("toggle-backdrop-blur")
                .on_click(move || toggle.set(!enabled)),
            );

        div()
            .items_center()
            .justify_center()
            .bg(Color::rgb(0.025, 0.035, 0.065))
            .child(
                // Borders are paint-only in Anmixiu, so this child occupies the same bounds as its
                // parent and filters the vivid border/background transition underneath it.
                div()
                    .w(520.0)
                    .h(320.0)
                    .border(72.0)
                    .border_color(Color::rgb(0.95, 0.18, 0.52))
                    .bg(Color::rgb(0.05, 0.68, 0.92))
                    .rounded(36.0)
                    .child(panel),
            )
    }
}

impl Element for BlurShowcase {}

fn main() -> Result<(), anmixiu::AppError> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    App::new()
        .window(
            Window::new()
                .title("Anmixiu Backdrop Blur")
                .size(720.0, 520.0),
        )
        .run(BlurShowcase::default())
}
