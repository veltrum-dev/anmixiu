#[cfg(target_os = "macos")]
use std::{hint::black_box, sync::Arc};

#[cfg(target_os = "macos")]
use criterion::BenchmarkId;
use criterion::{Criterion, criterion_group, criterion_main};

#[cfg(target_os = "macos")]
fn split_backdrop_scene(
    size: anmixiu_scene::Size,
    blur_bounds: anmixiu_scene::Rect,
    sigma: f32,
    corner_radius: f32,
) -> anmixiu_scene::Scene {
    use anmixiu_scene::{Color, DrawCommand, Point, Rect, Scene, Size};

    Scene::new(
        vec![
            DrawCommand::SolidQuad {
                bounds: Rect::new(
                    Point::new(0.0, 0.0),
                    Size::new(size.width / 2.0, size.height),
                ),
                color: Color::rgba(1.0, 0.0, 0.0, 1.0),
                clip: None,
            },
            DrawCommand::SolidQuad {
                bounds: Rect::new(
                    Point::new(size.width / 2.0, 0.0),
                    Size::new(size.width / 2.0, size.height),
                ),
                color: Color::rgba(0.0, 0.0, 1.0, 1.0),
                clip: None,
            },
            DrawCommand::BackdropBlur {
                bounds: blur_bounds,
                sigma,
                corner_radius,
                clip: None,
            },
        ],
        Vec::new(),
        Vec::new(),
    )
}

#[cfg(target_os = "macos")]
fn filter_blur_scene() -> anmixiu_scene::Scene {
    use anmixiu_scene::{Color, DrawCommand, Point, Rect, Scene, Size};

    Scene::new(
        vec![DrawCommand::FilterBlur {
            sigma: 10.0,
            clip: None,
            commands: Arc::from([
                DrawCommand::SolidQuad {
                    bounds: Rect::new(Point::new(48.0, 48.0), Size::new(80.0, 160.0)),
                    color: Color::rgba(1.0, 0.0, 0.0, 1.0),
                    clip: None,
                },
                DrawCommand::SolidQuad {
                    bounds: Rect::new(Point::new(128.0, 48.0), Size::new(80.0, 160.0)),
                    color: Color::rgba(0.0, 0.0, 1.0, 1.0),
                    clip: None,
                },
            ]),
        }],
        Vec::new(),
        Vec::new(),
    )
}

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
    let blur_scene = split_backdrop_scene(
        Size::new(256.0, 256.0),
        Rect::new(Point::new(48.0, 48.0), Size::new(160.0, 160.0)),
        16.0,
        24.0,
    );
    group.bench_with_input(
        BenchmarkId::new("backdrop_blur_sigma", 16),
        &blur_scene,
        |bencher, scene| {
            bencher.iter(|| {
                black_box(renderer.render_offscreen(black_box(scene), size).unwrap());
            });
        },
    );
    let filter_scene = filter_blur_scene();
    group.bench_with_input(
        BenchmarkId::new("filter_blur_sigma", 10),
        &filter_scene,
        |bencher, scene| {
            bencher.iter(|| {
                black_box(renderer.render_offscreen(black_box(scene), size).unwrap());
            });
        },
    );
    group.finish();

    let retina_size = SurfaceSize::new(1_200, 800).unwrap();
    let direct_retina = Scene::new(
        vec![DrawCommand::SolidQuad {
            bounds: Rect::new(Point::new(0.0, 0.0), Size::new(600.0, 400.0)),
            color: Color::rgba(0.2, 0.4, 0.8, 1.0),
            clip: None,
        }],
        Vec::new(),
        Vec::new(),
    );
    let blur_retina = split_backdrop_scene(
        Size::new(600.0, 400.0),
        Rect::new(Point::new(0.0, 0.0), Size::new(600.0, 400.0)),
        16.0,
        24.0,
    );
    let mut retina_group = criterion.benchmark_group("metal_retina_600x400");
    retina_group.sample_size(20);
    for (name, scene) in [
        ("direct", direct_retina),
        ("backdrop_blur_sigma_16", blur_retina),
    ] {
        retina_group.bench_with_input(name, &scene, |bencher, scene| {
            bencher.iter(|| {
                black_box(
                    renderer
                        .render_offscreen_scaled(black_box(scene), retina_size, 2.0)
                        .unwrap(),
                );
            });
        });
    }
    retina_group.finish();
}

#[cfg(not(target_os = "macos"))]
fn register(_criterion: &mut Criterion) {}

criterion_group!(benches, register);
criterion_main!(benches);
