use anmixiu_core::{SharedString, shared_format};
use criterion::{Criterion, criterion_group, criterion_main};

fn shared_string_benchmarks(criterion: &mut Criterion) {
    let long = "a label that is deliberately longer than twenty-three bytes";
    let owned = long.to_owned();
    let shared = SharedString::from(long);

    criterion.bench_function("string/clone_long", |bencher| {
        bencher.iter(|| owned.clone());
    });
    criterion.bench_function("shared_string/clone_long", |bencher| {
        bencher.iter(|| shared.clone());
    });
    criterion.bench_function("string/format_counter", |bencher| {
        bencher.iter(|| format!("Count {}", 42));
    });
    criterion.bench_function("shared_string/format_counter", |bencher| {
        bencher.iter(|| shared_format!("Count {}", 42));
    });
}

criterion_group!(benches, shared_string_benchmarks);
criterion_main!(benches);
