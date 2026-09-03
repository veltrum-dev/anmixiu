use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, StatsAlloc};
#[cfg(target_os = "macos")]
use stats_alloc::{Region, Stats};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[cfg(target_os = "macos")]
fn main() {
    use anmixiu_render_metal::{MetalRenderer, SurfaceSize};
    use anmixiu_scene::{Color, DrawCommand, Point, Rect, Scene, Size};

    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        println!("Metal allocation probe unavailable: no Metal device");
        return;
    };
    let size = SurfaceSize::new(256, 256).unwrap();
    for command_count in [1_usize, 1_000] {
        let commands = (0..command_count)
            .map(|index| {
                let column = u16::try_from(index % 32).unwrap();
                let row = u16::try_from((index / 32) % 32).unwrap();
                DrawCommand::SolidQuad {
                    bounds: Rect::new(
                        Point::new(f32::from(column) * 8.0, f32::from(row) * 8.0),
                        Size::new(7.0, 7.0),
                    ),
                    color: Color::rgba(0.2, 0.5, 0.9, 1.0),
                    clip: None,
                }
            })
            .collect();
        let scene = Scene::new(commands, Vec::new(), Vec::new());
        drop(renderer.render_offscreen(&scene, size).unwrap());
        let region = Region::new(GLOBAL);
        drop(renderer.render_offscreen(&scene, size).unwrap());
        report(
            &format!("draw_commands_{command_count}"),
            1,
            region.change(),
            renderer.stats().cached_atlas_bytes,
            renderer.stats().compositor_texture_bytes,
        );

        let region = Region::new(GLOBAL);
        for _ in 0..100 {
            drop(renderer.render_offscreen(&scene, size).unwrap());
        }
        report(
            &format!("draw_commands_{command_count}"),
            100,
            region.change(),
            renderer.stats().cached_atlas_bytes,
            renderer.stats().compositor_texture_bytes,
        );
    }
    let blur_scene = Scene::new(
        vec![
            DrawCommand::SolidQuad {
                bounds: Rect::new(Point::new(0.0, 0.0), Size::new(128.0, 256.0)),
                color: Color::rgba(1.0, 0.0, 0.0, 1.0),
                clip: None,
            },
            DrawCommand::SolidQuad {
                bounds: Rect::new(Point::new(128.0, 0.0), Size::new(128.0, 256.0)),
                color: Color::rgba(0.0, 0.0, 1.0, 1.0),
                clip: None,
            },
            DrawCommand::BackdropBlur {
                bounds: Rect::new(Point::new(48.0, 48.0), Size::new(160.0, 160.0)),
                sigma: 16.0,
                corner_radius: 24.0,
                clip: None,
            },
        ],
        Vec::new(),
        Vec::new(),
    );
    drop(renderer.render_offscreen(&blur_scene, size).unwrap());
    let region = Region::new(GLOBAL);
    for _ in 0..100 {
        drop(renderer.render_offscreen(&blur_scene, size).unwrap());
    }
    report(
        "backdrop_blur_sigma_16",
        100,
        region.change(),
        renderer.stats().cached_atlas_bytes,
        renderer.stats().compositor_texture_bytes,
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("Metal allocation probe unavailable: non-macOS host");
}

#[cfg(target_os = "macos")]
fn report(
    workload: &str,
    iterations: usize,
    stats: Stats,
    resident_atlas_bytes: usize,
    resident_compositor_bytes: usize,
) {
    println!(
        "workload={workload},iterations={iterations},allocations={},bytes_allocated={},deallocations={},bytes_deallocated={},reallocations={},bytes_reallocated={},resident_atlas_bytes={resident_atlas_bytes},resident_compositor_bytes={resident_compositor_bytes}",
        stats.allocations,
        stats.bytes_allocated,
        stats.deallocations,
        stats.bytes_deallocated,
        stats.reallocations,
        stats.bytes_reallocated,
    );
}
