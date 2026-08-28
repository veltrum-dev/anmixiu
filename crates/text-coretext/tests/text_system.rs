#![cfg(target_os = "macos")]

use anmixiu_scene::{AtlasId, Point};
use anmixiu_text_coretext::{AtlasConfig, FontSpec, TextSystem};
use core_foundation::{
    attributed_string::CFMutableAttributedString,
    base::{CFRange, TCFType},
    string::CFString,
};
use core_text::{
    font::{self, CTFont, kCTFontSystemFontType},
    font_descriptor::kCTFontOrientationHorizontal,
    line::CTLine,
};

#[test]
fn system_ui_font_uses_core_texts_current_macos_font() {
    let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let expected =
        font::new_ui_font_for_language(kCTFontSystemFontType, 18.0, None).postscript_name();
    let shaped = text
        .shape("Anmixiu 中文", Point::default(), &FontSpec::system_ui(18.0))
        .unwrap();

    assert_eq!(
        shaped.fonts.first().map(String::as_str),
        Some(expected.as_str()),
        "the default UI run must follow the current macOS system font instead of a legacy hard-coded family"
    );
    assert_eq!(shaped.glyphs.len(), 9);
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn zero_sentinel_resolves_to_the_visible_platform_ui_font_size() {
    let expected_font = font::new_ui_font_for_language(kCTFontSystemFontType, 0.0, None);
    let expected_size = expected_font.pt_size() as f32;
    assert!(expected_size > 0.0);

    let mut defaults = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let default_shape = defaults
        .shape(
            "System default",
            Point::default(),
            &FontSpec::system_ui_default(),
        )
        .unwrap();
    let mut explicit = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let explicit_shape = explicit
        .shape(
            "System default",
            Point::default(),
            &FontSpec::system_ui(expected_size),
        )
        .unwrap();

    assert!((default_shape.metrics.width - explicit_shape.metrics.width).abs() < 0.001);
    assert!((default_shape.metrics.height - explicit_shape.metrics.height).abs() < 0.001);
}

#[test]
fn core_text_measures_english_and_chinese_with_fallback() {
    let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let font = FontSpec::new("Helvetica", 18.0);

    let english = text
        .shape("Hello", Point { x: 0.0, y: 0.0 }, &font)
        .unwrap();
    let chinese = text.shape("你好", Point { x: 0.0, y: 0.0 }, &font).unwrap();

    assert!(english.metrics.width > 0.0);
    assert!(english.metrics.ascent > 0.0);
    assert_eq!(english.glyphs.len(), 5);
    assert!(chinese.metrics.width > 0.0);
    assert_eq!(chinese.glyphs.len(), 2);
    assert!(
        chinese.fonts.iter().any(|name| name != "Helvetica"),
        "CoreText should select a fallback font for Chinese"
    );
}

#[test]
fn rasterized_glyphs_publish_non_empty_r8_atlas_data() {
    let mut text = TextSystem::new(AtlasConfig::new(128, 128, 32)).unwrap();
    let shaped = text
        .shape(
            "A你",
            Point { x: 4.0, y: 6.0 },
            &FontSpec::new("Helvetica", 24.0),
        )
        .unwrap();
    let upload = shaped.atlas_upload.expect("first use uploads the atlas");

    assert_eq!(upload.atlas, AtlasId::TEXT);
    assert_eq!(upload.pixels.len(), 128 * 128);
    assert!(upload.pixels.iter().any(|alpha| *alpha != 0));
    assert!(shaped.glyphs.iter().all(|glyph| {
        glyph.uv_bounds.origin.x >= 0.0
            && glyph.uv_bounds.origin.y >= 0.0
            && glyph.uv_bounds.max_x() <= 1.0
            && glyph.uv_bounds.max_y() <= 1.0
    }));
}

#[test]
fn atlas_reuses_glyphs_and_never_exceeds_its_hard_capacity() {
    let mut text = TextSystem::new(AtlasConfig::new(128, 128, 4)).unwrap();
    let font = FontSpec::new("Helvetica", 16.0);
    let first = text.shape("AB", Point::default(), &font).unwrap();
    let first_generation = first.atlas_upload.unwrap().generation;
    let rasterized_after_first = text.rasterized_glyph_count();
    let reused = text.shape("AB", Point::default(), &font).unwrap();

    assert!(
        reused.atlas_upload.is_none(),
        "unchanged atlas is not uploaded"
    );
    assert_eq!(text.atlas_len(), 2);
    assert_eq!(
        text.rasterized_glyph_count(),
        rasterized_after_first,
        "atlas hits must bypass CoreGraphics rasterization"
    );

    let replaced = text.shape("CDEF", Point::default(), &font).unwrap();
    let second_generation = replaced.atlas_upload.unwrap().generation;
    assert!(second_generation > first_generation);
    assert!(text.atlas_len() <= 4);
    let stats = text.atlas_stats();
    assert_eq!(stats.entry_capacity, 4);
    assert_eq!(stats.resident_bytes, 128 * 128);

    for _ in 0..100 {
        let _ = text.shape("CDEF", Point::default(), &font).unwrap();
    }
    assert_eq!(text.atlas_len(), 4, "warm steady state must not grow");
}

#[test]
fn atlas_repack_counter_advances_only_when_existing_glyphs_move() {
    let mut text = TextSystem::new(AtlasConfig::new(128, 128, 4)).unwrap();
    let font = FontSpec::new("Helvetica", 16.0);

    let start = text.atlas_repacks();
    text.shape("AB", Point::default(), &font).unwrap();
    assert_eq!(
        text.atlas_repacks(),
        start,
        "first fill is an incremental append, not a repack"
    );

    // Re-shaping resident glyphs touches nothing.
    text.shape("AB", Point::default(), &font).unwrap();
    assert_eq!(text.atlas_repacks(), start, "atlas hits never repack");

    // "CDEF" needs 4 new glyphs on top of the 2 resident ones; that exceeds capacity 4 and forces
    // a clear + repack, moving the surviving glyphs to fresh positions.
    text.shape("CDEF", Point::default(), &font).unwrap();
    assert_eq!(
        text.atlas_repacks(),
        start + 1,
        "capacity overflow repacks exactly once"
    );
    assert_eq!(text.atlas_stats().repacks, start + 1);

    // A warm steady state over the same glyphs must not repack again.
    for _ in 0..50 {
        text.shape("CDEF", Point::default(), &font).unwrap();
    }
    assert_eq!(
        text.atlas_repacks(),
        start + 1,
        "warm steady state does not repack"
    );
}

#[test]
fn invalid_atlas_configuration_is_structured_error() {
    let error = TextSystem::new(AtlasConfig::new(0, 128, 10)).unwrap_err();
    assert!(error.to_string().contains("width"));
}

#[test]
fn scale_transitions_preserve_complete_chinese_punctuation_and_metrics() {
    let mut text = TextSystem::new(AtlasConfig::new(512, 512, 256)).unwrap();
    let font = FontSpec::new("Helvetica", 18.0);
    let value = "同一事件连续更新 3 次（合并到下一帧）";
    let visible_chars = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();

    for scale in [2.0, 1.0, 2.0] {
        let shaped = text
            .shape_scaled(value, Point::default(), &font, scale)
            .unwrap();
        assert_eq!(
            shaped.glyphs.len(),
            visible_chars,
            "scale {scale} dropped a visible glyph"
        );
        let final_glyph = shaped
            .glyphs
            .last()
            .expect("closing punctuation is present");
        assert!(final_glyph.bounds.max_x() <= shaped.metrics.width + 1.0);
    }
}

#[test]
fn glyph_quads_align_one_to_one_with_physical_pixels_at_each_scale() {
    let mut text = TextSystem::new(AtlasConfig::new(512, 512, 256)).unwrap();
    let font = FontSpec::new("Helvetica", 18.0);

    for scale in [1.0, 2.0] {
        let shaped = text
            .shape_scaled("CoreText 中文清晰", Point::default(), &font, scale)
            .unwrap();
        for glyph in shaped.glyphs.iter() {
            for physical in [
                glyph.bounds.origin.x * scale,
                glyph.bounds.origin.y * scale,
                glyph.bounds.size.width * scale,
                glyph.bounds.size.height * scale,
            ] {
                assert!(
                    (physical - physical.round()).abs() < 0.001,
                    "scale {scale} produced a fractional physical glyph edge: {physical}"
                );
            }
        }
    }
}

#[test]
fn fractional_x_positions_get_distinct_glyph_masks_and_pixel_aligned_quads() {
    let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let font = FontSpec::system_ui(18.0);
    let whole = text
        .shape_scaled("A", Point::new(0.0, 0.0), &font, 1.0)
        .unwrap();
    let fractional = text
        .shape_scaled("A", Point::new(0.5, 0.0), &font, 1.0)
        .unwrap();

    assert_ne!(
        whole.glyphs[0].uv_bounds, fractional.glyphs[0].uv_bounds,
        "glyph caching must include the horizontal subpixel phase"
    );
    assert_eq!(text.atlas_len(), 2);
    for glyph in [whole.glyphs[0], fractional.glyphs[0]] {
        for edge in [
            glyph.bounds.origin.x,
            glyph.bounds.origin.y,
            glyph.bounds.size.width,
            glyph.bounds.size.height,
        ] {
            assert!(
                (edge - edge.round()).abs() < 0.001,
                "the textured quad remains 1:1"
            );
        }
    }
}

#[test]
fn mixed_glyph_bearings_share_one_pixel_aligned_baseline() {
    let value = "Ag中y";
    let scale = 1.0;
    let size = 18.0;
    let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let shaped = text
        .shape_scaled(value, Point::default(), &FontSpec::system_ui(size), scale)
        .unwrap();
    let expected_tops = expected_pixel_aligned_tops(value, size, shaped.metrics.ascent, scale);

    assert_eq!(shaped.glyphs.len(), expected_tops.len());
    for (glyph, expected_top) in shaped.glyphs.iter().zip(expected_tops) {
        assert!(
            (f64::from(glyph.bounds.origin.y) - expected_top).abs() < 0.001,
            "glyph top {} does not preserve the common rounded baseline {expected_top}",
            glyph.bounds.origin.y
        );
    }
}

fn expected_pixel_aligned_tops(value: &str, size: f32, ascent: f32, scale: f32) -> Vec<f64> {
    const PADDING: f64 = 2.0;
    let base_font = font::new_ui_font_for_language(kCTFontSystemFontType, f64::from(size), None);
    let mut attributed = CFMutableAttributedString::new();
    attributed.replace_str(&CFString::new(value), CFRange::init(0, 0));
    let font_key = CFString::new("NSFont");
    attributed.set_attribute(
        CFRange::init(0, attributed.char_len()),
        font_key.as_concrete_TypeRef(),
        &base_font,
    );
    let line = CTLine::new_with_attributed_string(attributed.as_concrete_TypeRef());
    let mut tops = Vec::new();
    for run in line.glyph_runs().iter() {
        let run_font = run
            .attributes()
            .and_then(|attributes| attributes.get(CFString::new("NSFont")).downcast::<CTFont>())
            .unwrap_or_else(|| base_font.clone());
        let scaled_font = run_font.clone_with_font_size(run_font.pt_size() * f64::from(scale));
        for (&glyph, &position) in run.glyphs().iter().zip(run.positions().iter()) {
            let bounds =
                scaled_font.get_bounding_rects_for_glyphs(kCTFontOrientationHorizontal, &[glyph]);
            if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
                continue;
            }
            let physical_baseline = (f64::from(ascent) - position.y) * f64::from(scale);
            let physical_top = (bounds.origin.y + bounds.size.height).ceil();
            tops.push((physical_baseline.round() - physical_top - PADDING) / f64::from(scale));
        }
    }
    tops
}

#[test]
fn glyph_uvs_include_a_transparent_safety_border_for_antialiasing() {
    let atlas_size = 256_u16;
    let mut text = TextSystem::new(AtlasConfig::new(
        u32::from(atlas_size),
        u32::from(atlas_size),
        128,
    ))
    .unwrap();
    let shaped = text
        .shape_scaled(
            "国MW",
            Point::default(),
            &FontSpec::new("Helvetica", 18.0),
            1.0,
        )
        .unwrap();
    let upload = shaped.atlas_upload.unwrap();

    for glyph in shaped.glyphs.iter() {
        let x = normalized_to_texel(glyph.uv_bounds.origin.x, atlas_size);
        let y = normalized_to_texel(glyph.uv_bounds.origin.y, atlas_size);
        let width = normalized_to_texel(glyph.uv_bounds.size.width, atlas_size);
        let height = normalized_to_texel(glyph.uv_bounds.size.height, atlas_size);
        assert!(width >= 3 && height >= 3);
        for column in x..x + width {
            assert_eq!(
                upload.pixels[(y * u32::from(atlas_size) + column) as usize],
                0
            );
            assert_eq!(
                upload.pixels[((y + height - 1) * u32::from(atlas_size) + column) as usize],
                0
            );
        }
        for row in y..y + height {
            assert_eq!(upload.pixels[(row * u32::from(atlas_size) + x) as usize], 0);
            assert_eq!(
                upload.pixels[(row * u32::from(atlas_size) + x + width - 1) as usize],
                0
            );
        }
    }
}

fn normalized_to_texel(value: f32, atlas_size: u16) -> u32 {
    (0..=atlas_size)
        .find(|texel| {
            (value - f32::from(*texel) / f32::from(atlas_size)).abs() < f32::EPSILON * 4.0
        })
        .map(u32::from)
        .expect("atlas UV coordinates are exact texel multiples")
}
