use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

#[cfg(target_os = "macos")]
fn register(criterion: &mut Criterion) {
    use anmixiu_scene::Point;
    use anmixiu_text_coretext::{AtlasConfig, FontSpec, TextSystem};

    let mut group = criterion.benchmark_group("coretext_shape");
    let font = FontSpec::system_ui(16.0);
    for (name, value) in [
        ("normal", "Counter 42 / Ready / 你好"),
        (
            "stress",
            "Anmixiu native GUI: English 中文 fallback 0123456789 ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ),
    ] {
        let mut text = TextSystem::new(AtlasConfig::new(1024, 1024, 2048)).unwrap();
        let _ = text.shape(value, Point::default(), &font).unwrap();
        group.bench_with_input(BenchmarkId::new("cached", name), value, |bencher, value| {
            bencher.iter(|| {
                black_box(
                    text.shape(black_box(value), Point::default(), black_box(&font))
                        .unwrap(),
                );
            });
        });
    }
    group.finish();
}

#[cfg(not(target_os = "macos"))]
fn register(_criterion: &mut Criterion) {}

criterion_group!(benches, register);
criterion_main!(benches);
