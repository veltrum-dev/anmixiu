use std::{alloc::System, rc::Rc};

use anmixiu_core::{Element, ParentElement, SharedString, div, text};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn main() {
    let iterations = 1_000;
    let literal = "static-label";
    report("String::from(static)", iterations, || {
        let value = String::from(literal);
        std::hint::black_box(value);
    });
    report("SharedString::new_static", iterations, || {
        let value = SharedString::new_static(literal);
        std::hint::black_box(value);
    });

    let long = "a label that is deliberately longer than twenty-three bytes";
    let owned = long.to_owned();
    let shared = SharedString::from(long);
    report("String::clone(long)", iterations, || {
        std::hint::black_box(owned.clone());
    });
    report("SharedString::clone(long)", iterations, || {
        std::hint::black_box(shared.clone());
    });

    let tree = div()
        .child(text("first"))
        .child(div().child(text(long)))
        .child(text("third"))
        .into_element_node();
    let snapshot = Rc::new(tree.clone());
    report("ElementNode::clone(tree)", iterations, || {
        std::hint::black_box(tree.clone());
    });
    report("Rc<ElementNode>::clone", iterations, || {
        std::hint::black_box(snapshot.clone());
    });
}

fn report(label: &str, iterations: usize, mut operation: impl FnMut()) {
    let region = Region::new(GLOBAL);
    for _ in 0..iterations {
        operation();
    }
    let stats = region.change();
    print_stats(label, iterations, stats);
}

fn print_stats(label: &str, iterations: usize, stats: Stats) {
    println!(
        "{label},{iterations},allocations={},bytes_allocated={},deallocations={},bytes_deallocated={},reallocations={},bytes_reallocated={}",
        stats.allocations,
        stats.bytes_allocated,
        stats.deallocations,
        stats.bytes_deallocated,
        stats.reallocations,
        stats.bytes_reallocated,
    );
}
