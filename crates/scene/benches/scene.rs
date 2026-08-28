use std::{hint::black_box, num::NonZeroUsize};

use anmixiu_scene::{Color, DrawCommand, Point, Rect, Scene, SceneCache, SceneCacheKey, Size};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn quad(index: usize) -> DrawCommand {
    let coordinate = f32::from(u16::try_from(index % 10_000).unwrap());
    DrawCommand::SolidQuad {
        bounds: Rect::new(Point::new(coordinate, coordinate), Size::new(20.0, 20.0)),
        color: Color::rgb(0.2, 0.4, 0.8),
        clip: None,
    }
}

fn scene_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("scene_build");
    for command_count in [100_usize, 10_000] {
        group.throughput(Throughput::Elements(command_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(command_count),
            &command_count,
            |b, &command_count| {
                b.iter(|| {
                    let commands = (0..command_count).map(quad).collect();
                    black_box(Scene::new(commands, Vec::new(), Vec::new()))
                });
            },
        );
    }
    group.finish();
}

fn scene_cache_hit(c: &mut Criterion) {
    let mut cache = SceneCache::new(NonZeroUsize::new(128).unwrap());
    let key = SceneCacheKey::new(1, 1, 1, 2.0);
    cache.get_or_insert_with(key, Scene::empty);

    c.bench_function("scene_cache/hit_steady_state", |b| {
        b.iter(|| black_box(cache.get_or_insert_with(key, Scene::empty)));
    });
}

criterion_group!(benches, scene_build, scene_cache_hit);
criterion_main!(benches);
