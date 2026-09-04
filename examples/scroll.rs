#![forbid(unsafe_code)]

//! A deliberately busy scroll surface used to exercise both trackpad axes.

use anmixiu::prelude::*;

const ROW_COUNT: usize = 120;
const CONTENT_WIDTH: f32 = 1_520.0;

#[derive(Default)]
struct ScrollDemo {
    surface: ScrollHandle,
}

impl Render for ScrollDemo {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = (0..ROW_COUNT).map(|row| {
            div()
                .flex_row()
                .w(CONTENT_WIDTH)
                .min_w(CONTENT_WIDTH)
                .h(px(42.0))
                .gap(px(16.0))
                .p(px(10.0))
                .bg(if row % 2 == 0 {
                    Color::rgb(0.08, 0.11, 0.18)
                } else {
                    Color::rgb(0.06, 0.09, 0.15)
                })
                .rounded(px(8.0))
                .child(
                    text(shared_format!("第 {:03} 行", row + 1))
                        .w(px(112.0))
                        .text_color(Color::rgb(0.64, 0.78, 1.0)),
                )
                .child(
                    text(shared_format!(
                        "横向拖动可以看到这一行后面的更多字段 · batch-{:03} · latency {:02} ms",
                        row + 1,
                        8 + (row * 7) % 43
                    ))
                    .w(px(920.0))
                    .text_color(Color::rgb(0.84, 0.88, 0.95)),
                )
                .child(
                    text(shared_format!(
                        "状态 {}",
                        if row % 3 == 0 {
                            "进行中"
                        } else {
                            "已完成"
                        }
                    ))
                    .w(px(180.0))
                    .text_color(Color::rgb(0.58, 0.86, 0.7)),
                )
        });

        div()
            .p(px(24.0))
            .gap(px(14.0))
            .bg(Color::rgb(0.025, 0.035, 0.065))
            .text_color(Color::rgb(0.9, 0.93, 1.0))
            .child(text("双轴滚动示例").text_color(Color::rgb(0.65, 0.8, 1.0)))
            .child(text("滚轮/触控板可纵向滚动；触控板横向手势可查看右侧字段。滚动过程按显示器刷新率平滑追踪。"))
            .child(
                div()
                    // Keep a real viewport so the long list overflows instead of making the
                    // container grow to its full content height.
                    .h(px(500.0))
                    .scroll(&self.surface)
                    .bg(Color::rgb(0.045, 0.06, 0.1))
                    .rounded(px(12.0))
                    .child(
                        div()
                            .w(CONTENT_WIDTH)
                            .min_w(CONTENT_WIDTH)
                            .gap(px(8.0))
                            .p(px(14.0))
                            .children(rows),
                    ),
            )
    }
}

fn main() -> Result<(), anmixiu::AppError> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    App::new()
        .window(
            Window::new()
                .title("Anmixiu Smooth Scroll")
                .size(960.0, 720.0),
        )
        .run(ScrollDemo::default())
}
