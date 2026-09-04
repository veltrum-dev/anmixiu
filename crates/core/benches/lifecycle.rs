use std::{cell::Cell, rc::Rc};

use anmixiu_core::{
    AppStateStore, Context, DivElement, Element, ElementHost, IntoElement, Lifecycle,
    ParentElement, Style, Styled, WindowStateStore, div, text,
};
use anmixiu_reactive::Signal;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

struct LabelElement {
    style: Style,
    value: Signal<u32>,
}

impl Styled for LabelElement {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }

    fn style_ref(&self) -> &Style {
        &self.style
    }
}

impl Lifecycle for LabelElement {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        text(self.value.get().to_string())
    }
}

impl Element for LabelElement {}

struct LabelTree {
    root: DivElement,
}

impl Lifecycle for LabelTree {
    fn render(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        self.root.clone()
    }
}

fn mounted_labels(
    count: usize,
) -> (
    ElementHost<LabelTree>,
    anmixiu_reactive::OwnerRegistry,
    Signal<u32>,
) {
    let target = Signal::new(0);
    let mut root = div();
    for index in 0..count {
        root = root.child(LabelElement {
            style: Style::default(),
            value: if index + 1 == count {
                target.clone()
            } else {
                Signal::new(0)
            },
        });
    }
    let context = Context::testing_with_state(AppStateStore::new(), WindowStateStore::new());
    let owners = context.owner_registry().clone();
    let mut host = ElementHost::new(Rc::new(LabelTree { root }), context);
    host.render().expect("initial lifecycle tree");
    host.did_paint();
    (host, owners, target)
}

fn lifecycle_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("lifecycle_dirty_route");
    for count in [1_usize, 100, 10_000] {
        group.throughput(Throughput::Elements(1));
        let (mut host, owners, target) = mounted_labels(count);
        let next = Cell::new(1_u32);
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |bencher, _| {
            bencher.iter(|| {
                let value = next.get();
                next.set(value.wrapping_add(1));
                target.set(value);
                let dirty = owners.take_dirty();
                std::hint::black_box(host.render_dirty(&dirty).expect("dirty Element renders"));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, lifecycle_benchmarks);
criterion_main!(benches);
