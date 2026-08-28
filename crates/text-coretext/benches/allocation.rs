use std::alloc::System;

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[cfg(target_os = "macos")]
fn main() {
    use anmixiu_scene::Point;
    use anmixiu_text_coretext::{AtlasConfig, FontSpec, TextSystem};

    let font = FontSpec::system_ui(16.0);
    for (label, value) in [
        ("normal", "Counter 42 / Ready / 你好"),
        (
            "stress",
            "Anmixiu native GUI: English 中文 fallback 0123456789 ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ),
    ] {
        let mut text = TextSystem::new(AtlasConfig::new(1024, 1024, 2048)).unwrap();
        drop(text.shape(value, Point::default(), &font).unwrap());
        let region = Region::new(GLOBAL);
        drop(text.shape(value, Point::default(), &font).unwrap());
        report(label, 1, region.change(), text.atlas_stats().resident_bytes);

        let region = Region::new(GLOBAL);
        for _ in 0..1_000 {
            drop(text.shape(value, Point::default(), &font).unwrap());
        }
        report(
            label,
            1_000,
            region.change(),
            text.atlas_stats().resident_bytes,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("CoreText allocation probe unavailable: non-macOS host");
}

fn report(label: &str, iterations: usize, stats: Stats, resident_bytes: usize) {
    println!(
        "{label},{iterations},allocations={},bytes_allocated={},deallocations={},bytes_deallocated={},reallocations={},bytes_reallocated={},resident_bytes={resident_bytes}",
        stats.allocations,
        stats.bytes_allocated,
        stats.deallocations,
        stats.bytes_deallocated,
        stats.reallocations,
        stats.bytes_reallocated,
    );
}
