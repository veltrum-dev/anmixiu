use anmixiu_core::{Element, GlobalElementId, InteractiveElement, ParentElement, button, div};
use anmixiu_platform_macos::FrameBuilder;
use anmixiu_scene::Size;
use criterion::{Criterion, criterion_group, criterion_main};

fn element_tree(count: u64) -> anmixiu_core::ElementNode {
    (0..count)
        .fold(div(), |root, index| {
            root.child(button("Button").id(("button", index)))
        })
        .into_element_node()
}

fn frame_builder_benchmarks(criterion: &mut Criterion) {
    let tree = element_tree(100);
    let viewport = Size::new(800.0, 600.0);

    let mut cached = FrameBuilder::new().unwrap();
    cached.build(&tree, viewport, 2.0).unwrap();
    criterion.bench_function("frame_builder/100_cached", |bencher| {
        bencher.iter(|| cached.build(&tree, viewport, 2.0).unwrap());
    });

    let mut hovered = FrameBuilder::new().unwrap();
    hovered.build(&tree, viewport, 2.0).unwrap();
    let first = GlobalElementId::new([("button", 1_u64).into()]);
    let second = GlobalElementId::new([("button", 2_u64).into()]);
    let mut alternate = false;
    criterion.bench_function("frame_builder/100_hover_toggle", |bencher| {
        bencher.iter(|| {
            alternate = !alternate;
            hovered.set_hovered(Some(if alternate {
                first.clone()
            } else {
                second.clone()
            }));
            hovered.build(&tree, viewport, 2.0).unwrap()
        });
    });
}

criterion_group!(benches, frame_builder_benchmarks);
criterion_main!(benches);
