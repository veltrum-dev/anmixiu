use anmixiu::prelude::*;

#[derive(Default)]
struct MainWindow {
    style: Style,
    next_detail: Signal<u64>,
}

impl Styled for MainWindow {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }
    fn style_ref(&self) -> &Style {
        &self.style
    }
}

impl Lifecycle for MainWindow {
    fn render(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.app();
        let current = cx.window();
        let info = current.info();
        let next_detail = self.next_detail.clone();
        let rename_window = current.clone();
        let reset_window = current;

        div()
            .padding(20)
            .gap(12)
            .child(text(shared_format!(
                "Main {:?}: {} × {}, scale {:.2}",
                info.id,
                info.content_size.width().value(),
                info.content_size.height().value(),
                info.scale_factor,
            )))
            .child(
                button("Open detail window")
                    .id("open-detail")
                    .on_click(move || {
                        let detail = next_detail.get() + 1;
                        next_detail.set(detail);
                        if let Err(error) = app.open_window(
                            Window::new()
                                .title(shared_format!("Detail {detail}"))
                                .size(420.0, 260.0),
                            DetailWindow {
                                style: Style::default(),
                                detail,
                            },
                        ) {
                            eprintln!("failed to open detail window: {error}");
                        }
                    }),
            )
            .child(
                button("Rename and resize this window")
                    .id("update-main-window")
                    .on_click(move || {
                        if let Err(error) = rename_window.update(
                            WindowUpdate::new()
                                .title("Updated main window")
                                .content_size(720.0, 520.0),
                        ) {
                            eprintln!("failed to update main window: {error}");
                        }
                    }),
            )
            .child(
                button("Reset title to app name")
                    .id("reset-main-title")
                    .on_click(move || {
                        if let Err(error) = reset_window.update(WindowUpdate::new().reset_title()) {
                            eprintln!("failed to reset main window title: {error}");
                        }
                    }),
            )
    }
}

impl Element for MainWindow {}

struct DetailWindow {
    style: Style,
    detail: u64,
}

impl Styled for DetailWindow {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }
    fn style_ref(&self) -> &Style {
        &self.style
    }
}

impl Lifecycle for DetailWindow {
    fn render(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let window = cx.window();
        let close_window = window.clone();
        let info = window.info();

        div()
            .padding(20)
            .gap(12)
            .child(text(shared_format!("Detail window #{}", self.detail)))
            .child(text(shared_format!(
                "id={:?}, focused={}, status={:?}",
                info.id,
                info.focused,
                info.status,
            )))
            .child(
                button("Close this window")
                    .id("close-detail")
                    .on_click(move || {
                        if let Err(error) = close_window.close() {
                            eprintln!("failed to close detail window: {error}");
                        }
                    }),
            )
    }
}

impl Element for DetailWindow {}

fn main() -> Result<(), anmixiu::AppError> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    App::new()
        .name("Anmixiu Multi Window")
        .window(Window::new().size(620.0, 460.0))
        .run(MainWindow::default())
}
