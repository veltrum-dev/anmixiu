#![forbid(unsafe_code)]

use anmixiu::prelude::*;

fn main() -> Result<(), anmixiu::AppError> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let showcase = div()
        .w(560.0)
        .h(360.0)
        .p(28.0)
        .gap(16.0)
        .items_center()
        .justify_center()
        .bg(Color::rgb(0.025, 0.035, 0.065))
        .text_color(Color::WHITE)
        .child(text("Tailwind-style aliases").text_color(Color::rgb(0.65, 0.8, 1.0)))
        .child(text(
            "These names are feature-gated aliases over the typed builders.",
        ))
        .child(
            button("A button using aliases")
                .w(280.0)
                .h(48.0)
                .bg(Color::rgb(0.2, 0.4, 0.8))
                .text_color(Color::WHITE)
                .rounded(10.0),
        );

    App::new()
        .window(
            Window::new()
                .title("Anmixiu Tailwind Aliases")
                .size(700.0, 500.0),
        )
        .run(showcase)
}
