use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

#[cfg(target_os = "macos")]
fn register(criterion: &mut Criterion) {
    use anmixiu_render_metal::{MetalRenderer, SurfaceSize};
    use anmixiu_scene::{Color, DrawCommand, Point, Rect, Scene, Size};

    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal benchmark skipped: no Metal device");
        return;
    };
    let size = SurfaceSize::new(256, 256).unwrap();
    let mut group = criterion.benchmark_group("metal_submit_wait_readback");
    group.sample_size(20);
    for command_count in [1_usize, 1_000] {
        let commands = (0..command_count)
            .map(|index| {
                let column = u16::try_from(index % 32).unwrap();
                let row = u16::try_from((index / 32) % 32).unwrap();
                let x = f32::from(column) * 8.0;
                let y = f32::from(row) * 8.0;
                DrawCommand::SolidQuad {
                    bounds: Rect::new(Point::new(x, y), Size::new(7.0, 7.0)),
                    color: Color::rgba(0.2, 0.5, 0.9, 1.0),
                    clip: None,
                }
            })
            .collect();
        let scene = Scene::new(commands, Vec::new(), Vec::new());
        group.bench_with_input(
            BenchmarkId::new("draw_commands", command_count),
            &scene,
            |bencher, scene| {
                bencher.iter(|| {
                    black_box(renderer.render_offscreen(black_box(scene), size).unwrap());
                });
            },
        );
    }
    group.finish();
}

#[cfg(not(target_os = "macos"))]
fn register(_criterion: &mut Criterion) {}

criterion_group!(benches, register);
criterion_main!(benches);
