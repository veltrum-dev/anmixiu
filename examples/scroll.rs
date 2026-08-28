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
                .width(CONTENT_WIDTH)
                .min_width(CONTENT_WIDTH)
                .height(px(42.0))
                .gap(px(16.0))
                .padding(px(10.0))
                .background(if row % 2 == 0 {
                    Color::rgb(0.08, 0.11, 0.18)
                } else {
                    Color::rgb(0.06, 0.09, 0.15)
                })
                .rounded(px(8.0))
                .child(
                    text(shared_format!("第 {:03} 行", row + 1))
                        .width(px(112.0))
                        .foreground(Color::rgb(0.64, 0.78, 1.0)),
                )
                .child(
                    text(shared_format!(
                        "横向拖动可以看到这一行后面的更多字段 · batch-{:03} · latency {:02} ms",
                        row + 1,
                        8 + (row * 7) % 43
                    ))
                    .width(px(920.0))
                    .foreground(Color::rgb(0.84, 0.88, 0.95)),
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
                    .width(px(180.0))
                    .foreground(Color::rgb(0.58, 0.86, 0.7)),
                )
        });

        div()
            .padding(px(24.0))
            .gap(px(14.0))
            .background(Color::rgb(0.025, 0.035, 0.065))
            .foreground(Color::rgb(0.9, 0.93, 1.0))
            .child(text("双轴滚动示例").foreground(Color::rgb(0.65, 0.8, 1.0)))
            .child(text("滚轮/触控板可纵向滚动；触控板横向手势可查看右侧字段。滚动过程按显示器刷新率平滑追踪。"))
            .child(
                div()
                    // Keep a real viewport so the long list overflows instead of making the
                    // container grow to its full content height.
                    .height(px(500.0))
                    .scroll(&self.surface)
                    .background(Color::rgb(0.045, 0.06, 0.1))
                    .rounded(px(12.0))
                    .child(
                        div()
                            .width(CONTENT_WIDTH)
                            .min_width(CONTENT_WIDTH)
                            .gap(px(8.0))
                            .padding(px(14.0))
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
                .size(960.0, 720.0)
                .font_size(px(16.0)),
        )
        .run(ScrollDemo::default())
}
