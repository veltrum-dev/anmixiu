use std::hint::black_box;

use anmixiu_layout_taffy::{
    Dimension, FlexDirection, LayoutEngine, LayoutNode, LayoutNodeId, LayoutRequest,
    LayoutRevisions, LayoutStyle, Viewport,
};
use anmixiu_scene::Size;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn tree(node_count: usize) -> LayoutNode {
    let mut root = LayoutNode::new(LayoutNodeId(0)).with_style(LayoutStyle {
        width: Dimension::Points(1_000.0),
        direction: FlexDirection::Column,
        ..LayoutStyle::default()
    });
    for id in 1..node_count {
        root = root.with_child(
            LayoutNode::new(LayoutNodeId(id as u64)).with_style(LayoutStyle {
                height: Dimension::Points(2.0),
                flex_shrink: 0.0,
                ..LayoutStyle::default()
            }),
        );
    }
    root
}

fn layout_uncached(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout_taffy_uncached");
    for node_count in [100_usize, 5_000] {
        let root = tree(node_count);
        let mut engine = LayoutEngine::new();
        let mut revision = 0_u64;
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(node_count),
            &node_count,
            |b, _| {
                b.iter(|| {
                    revision = revision.wrapping_add(1);
                    let result = engine
                        .compute(
                            LayoutRequest::new(
                                &root,
                                Viewport::new(Size::new(1_000.0, 10_000.0), 2.0),
                                LayoutRevisions::new(1, revision, 1),
                            ),
                            |_, _| Size::default(),
                        )
                        .unwrap();
                    black_box(result)
                });
            },
        );
    }
    group.finish();
}

fn layout_cached(c: &mut Criterion) {
    let root = tree(1_000);
    let request = || {
        LayoutRequest::new(
            &root,
            Viewport::new(Size::new(1_000.0, 10_000.0), 2.0),
            LayoutRevisions::new(1, 1, 1),
        )
    };
    let mut engine = LayoutEngine::new();
    engine.compute(request(), |_, _| Size::default()).unwrap();
    c.bench_function("layout_taffy_cached/1000_steady_state", |b| {
        b.iter(|| black_box(engine.compute(request(), |_, _| Size::default()).unwrap()));
    });
}

criterion_group!(benches, layout_uncached, layout_cached);
criterion_main!(benches);
