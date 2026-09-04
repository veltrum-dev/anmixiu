#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use anmixiu::prelude::*;

#[derive(Default)]
struct AppState {
    username: SharedString,
}

#[derive(Default)]
struct Counter {
    style: Style,
    count: Signal<u32>,
    ready: Signal<bool>,
    animating: Signal<bool>,
    animation_start: Signal<Option<Instant>>,
}

/// Duration of the progress-bar sweep.
const ANIMATION: Duration = Duration::from_secs(12);

impl Counter {
    /// Advances the animation for this frame and returns progress in `0.0..=1.0`. While active it
    /// requests the next frame; on completion it stops requesting (and clears `animating`), so the
    /// loop ends on its own and the render-loop guard never trips (these frames are declared).
    fn tick_animation(&self, cx: &mut Context<Self>) -> f32 {
        if !self.animating.get() {
            return 0.0;
        }
        let Some(start) = self.animation_start.get() else {
            return 0.0;
        };
        let elapsed = start.elapsed();
        if elapsed >= ANIMATION {
            self.animating.set(false);
            self.animation_start.set(None);
            return 1.0;
        }
        cx.request_animation_frame();
        elapsed.as_secs_f32() / ANIMATION.as_secs_f32()
    }
}

impl Styled for Counter {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }
    fn style_ref(&self) -> &Style {
        &self.style
    }
}

impl Lifecycle for Counter {
    fn on_mount(&self, cx: &mut Context<Self>) {
        let ready = self.ready.clone();
        if let Err(error) = cx.spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            ready.set(true);
        }) {
            eprintln!("failed to schedule initial counter load: {error}");
        }
    }

    fn render(&self, cx: &mut Context<Self>) -> impl anmixiu::IntoElement {
        let State(app) = cx.state::<AppState>();
        let count = self.count.get();
        let ready = self.ready.get();

        let sync_count = self.count.clone();
        let async_count = self.count.clone();
        let burst_count = self.count.clone();

        // Continuous animation driven by request_animation_frame; see `tick_animation`.
        let animating = self.animating.get();
        let progress = self.tick_animation(cx);

        let toggle_animating = self.animating.clone();
        let toggle_start = self.animation_start.clone();

        div()
            .p(px(28.0))
            .gap(px(18.0))
            .bg(Color::rgb(0.035, 0.05, 0.09))
            .text_color(Color::WHITE)
            .child(
                text(shared_format!("你好，{}", app.username))
                    .text_color(Color::rgb(0.55, 0.7, 1.0)),
            )
            .when_else(
                ready,
                |this| this.child(text("Ready · 原生中英文字体已加载")),
                |this| this.child(text("Loading…")),
            )
            .child(
                div()
                    .h(px(130.0))
                    .items_center()
                    .justify_center()
                    .bg(Color::rgb(0.075, 0.1, 0.17))
                    .rounded(px(18.0))
                    .child(
                        text(shared_format!("Count  {count}"))
                            .text_color(Color::rgb(0.95, 0.97, 1.0)),
                    ),
            )
            .child(
                div()
                    .flex_row()
                    .gap(px(12.0))
                    .child(
                        button("同步 +1")
                            .h(px(48.0))
                            .grow(1.0)
                            .id("sync-increment")
                            .on_click(move || sync_count.update(|value| *value += 1)),
                    )
                    .child(
                        button("异步延迟 +1")
                            .h(px(48.0))
                            .grow(1.0)
                            // .bg(Color::rgb(0.42, 0.24, 0.85))
                            // .text_color(Color::WHITE)
                            .id("async-increment")
                            .on_click(move || {
                                let count = async_count.clone();
                                async move {
                                    tokio::time::sleep(Duration::from_millis(250)).await;
                                    count.update(|value| *value += 1);
                                }
                            }),
                    ),
            )
            .child(
                button("同一事件连续更新 3 次（合并到下一帧）")
                    .h(px(44.0))
                    .bg(Color::rgb(0.12, 0.17, 0.26))
                    .text_color(Color::rgb(0.8, 0.86, 0.96))
                    .id("burst-increment")
                    .on_click(move || {
                        burst_count.update(|value| *value += 1);
                        burst_count.update(|value| *value += 1);
                        burst_count.update(|value| *value += 1);
                    }),
            )
            .child(
                button(if animating {
                    "动画进行中…"
                } else {
                    "播放进度动画（request_animation_frame）"
                })
                .h(px(44.0))
                .bg(Color::rgb(0.18, 0.13, 0.28))
                .text_color(Color::rgb(0.86, 0.82, 0.98))
                .id("play-animation")
                .on_click(move || {
                    toggle_start.set(Some(Instant::now()));
                    toggle_animating.set(true);
                }),
            )
            .child(progress_bar(progress))
    }
}

impl Element for Counter {}

/// A track with a fill whose width follows `progress` (`0.0..=1.0`).
fn progress_bar(progress: f32) -> DivElement {
    div()
        .h(px(14.0))
        .bg(Color::rgb(0.1, 0.13, 0.2))
        .rounded(px(7.0))
        .child(
            div()
                .h(px(14.0))
                .w(px(progress * 520.0))
                .rounded(px(7.0))
                .bg(Color::rgb(0.42, 0.6, 1.0)),
        )
}

fn main() -> Result<(), anmixiu::AppError> {
    // Route framework `tracing` events (e.g. the render-loop guard's warnings/errors) to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    App::new()
        .with_state(AppState {
            username: SharedString::new_static("Anmixiu 用户"),
        })
        .window(
            Window::new()
                .title("Anmixiu Counter MVP")
                .size(620.0, 520.0),
        )
        .run(Counter::default())
}
