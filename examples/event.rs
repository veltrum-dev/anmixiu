#![forbid(unsafe_code)]

use anmixiu::prelude::*;

#[derive(Clone, Copy)]
struct Ping;

#[derive(Default)]
struct Counter {
    count: Signal<u32>,
    dispatch_order: Signal<String>,
    subscription_count: Signal<usize>,
    subscription_summary: Signal<String>,
}

impl Eventful for Counter {
    fn bind_events(&self, _cx: &mut Context<Self>, bindings: &mut EventBindings) {
        let order = self.dispatch_order.clone();
        bindings.subscribe::<Ping, _>(EventScope::Window, EventPriority::HIGH, move |_| {
            order.update(|value| {
                if !value.is_empty() {
                    value.push_str(" → ");
                }
                value.push_str("high");
            });
        });

        let order = self.dispatch_order.clone();
        let count = self.count.clone();
        bindings.subscribe::<Ping, _>(EventScope::Window, EventPriority::NORMAL, move |_| {
            count.update(|value| *value += 1);
            order.update(|value| {
                if !value.is_empty() {
                    value.push_str(" → ");
                }
                value.push_str("normal");
            });
        });

        let order = self.dispatch_order.clone();
        bindings.subscribe::<Ping, _>(EventScope::Window, EventPriority::LOW, move |_| {
            order.update(|value| {
                if !value.is_empty() {
                    value.push_str(" → ");
                }
                value.push_str("low");
            });
        });
    }
}

impl Render for Counter {
    fn on_mount(&self, cx: &mut Context<Self>) {
        let events = cx.event_context();
        self.subscription_count.set(events.subscription_count());
        self.subscription_summary.set(
            events
                .subscriptions()
                .into_iter()
                .map(|subscription| {
                    format!(
                        "{} @ {}",
                        subscription.event_type,
                        subscription.priority.value()
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    fn render(&self, cx: &mut Context<Self>) -> impl anmixiu::IntoElement {
        let events = cx.event_context();
        let order = self.dispatch_order.clone();

        div()
            .p(px(28.0))
            .gap(px(16.0))
            .bg(Color::rgb(0.035, 0.05, 0.09))
            .text_color(Color::WHITE)
            .child(text("Eventful Element example"))
            .child(text(shared_format!(
                "Window subscriptions: {}",
                self.subscription_count.get()
            )))
            .child(text(shared_format!(
                "Registered: {}",
                self.subscription_summary.get()
            )))
            .child(text(shared_format!("Count: {}", self.count.get())))
            .child(text(shared_format!(
                "Dispatch order: {}",
                self.dispatch_order.get()
            )))
            .child(
                button("Emit Ping (Window scope)")
                    .id("emit-ping")
                    .on_click(move || {
                        order.set(String::new());
                        if let Err(error) = events.emit(Ping, EventScope::Window) {
                            eprintln!("failed to dispatch Ping: {error}");
                        }
                    }),
            )
            .child(text(
                "The same Eventful Element registered three listeners.",
            ))
    }
}

fn main() -> Result<(), anmixiu::AppError> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    App::new()
        .window(
            Window::new()
                .title("Anmixiu Eventful Element")
                .size(620.0, 360.0),
        )
        .run_eventful(Counter::default())
}
