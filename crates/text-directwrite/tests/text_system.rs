#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use std::{ffi::c_void, mem::size_of};

use anmixiu_scene::Point;
use anmixiu_text_directwrite::{AtlasConfig, FontSpec, ShapedText, TextError, TextSystem};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory,
};
use windows::Win32::UI::{
    HiDpi::SystemParametersInfoForDpi,
    WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS},
};
use windows::core::PCWSTR;

const LOGICAL_DPI: u32 = 96;

#[allow(clippy::cast_precision_loss)]
fn current_system_ui_font() -> (String, f32) {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: u32::try_from(size_of::<NONCLIENTMETRICSW>()).unwrap(),
        ..NONCLIENTMETRICSW::default()
    };
    // SAFETY: `metrics` is a correctly sized, writable NONCLIENTMETRICSW out parameter. A 96-DPI
    // query returns the logical-pixel font height consumed by DirectWrite.
    unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&raw mut metrics).cast::<c_void>()),
            0,
            LOGICAL_DPI,
        )
    }
    .unwrap();
    let face_name = &metrics.lfMessageFont.lfFaceName;
    let length = face_name
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(face_name.len());
    let family = String::from_utf16(&face_name[..length]).unwrap();
    let size = metrics.lfMessageFont.lfHeight.unsigned_abs() as f32 * (14.0 / 12.0);
    let mut font_collection = None;
    // SAFETY: DirectWrite initializes the valid out parameter and the shared factory remains
    // alive while the collection is queried.
    let factory: IDWriteFactory =
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.unwrap();
    unsafe { factory.GetSystemFontCollection(&raw mut font_collection, false) }.unwrap();
    let font_collection = font_collection.unwrap();
    let resolved_family = ["Segoe UI Variable", "Segoe UI"]
        .into_iter()
        .find(|candidate| {
            let wide = candidate
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let mut index = 0;
            let mut exists = windows::core::BOOL::default();
            // SAFETY: `wide` is nul-terminated and both out parameters are valid for this call.
            unsafe {
                font_collection
                    .FindFamilyName(PCWSTR(wide.as_ptr()), &raw mut index, &raw mut exists)
                    .is_ok()
                    && exists.as_bool()
            }
        })
        .map_or(family, str::to_owned);
    (resolved_family, size)
}

fn shape(font: &FontSpec) -> ShapedText {
    TextSystem::new(AtlasConfig::new(256, 256, 128))
        .unwrap()
        .shape("Anmixiu system UI", Point::default(), font)
        .unwrap()
}

fn assert_same_text_format(actual: &ShapedText, expected: &ShapedText) {
    assert_eq!(actual.fonts, expected.fonts);
    assert!((actual.metrics.width - expected.metrics.width).abs() < 0.001);
    assert!((actual.metrics.height - expected.metrics.height).abs() < 0.001);
    assert!((actual.metrics.ascent - expected.metrics.ascent).abs() < 0.001);
    assert!((actual.metrics.descent - expected.metrics.descent).abs() < 0.001);
}

#[test]
fn omitted_family_and_size_resolve_from_current_windows_ui_settings() {
    let (system_family, system_size) = current_system_ui_font();
    let explicit_system_font = FontSpec::new(system_family.clone(), system_size);

    assert_same_text_format(
        &shape(&FontSpec::system_ui_default()),
        &shape(&explicit_system_font),
    );
    assert_same_text_format(
        &shape(&FontSpec::system_ui(18.0)),
        &shape(&FontSpec::new(system_family.clone(), 18.0)),
    );
    assert_same_text_format(
        &shape(&FontSpec::named_default(system_family)),
        &shape(&explicit_system_font),
    );
}

#[test]
fn directwrite_shapes_latin_and_cjk_into_a_complete_alpha_atlas() {
    let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let shaped = text
        .shape(
            "Anmixiu 中文",
            Point::new(12.25, 8.0),
            &FontSpec::system_ui(18.0),
        )
        .unwrap();

    assert!(shaped.metrics.width > 0.0);
    assert!(shaped.metrics.height > 0.0);
    assert!(!shaped.glyphs.is_empty());
    assert!(!shaped.fonts.is_empty());
    let upload = shaped.atlas_upload.expect("first shape uploads the atlas");
    assert_eq!(upload.pixels.len(), 256 * 256);
    assert!(upload.pixels.iter().any(|alpha| *alpha != 0));
}

#[test]
fn identical_positioned_shape_reuses_the_bounded_atlas() {
    let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let font = FontSpec::system_ui(18.0);
    let first = text
        .shape_scaled("Reuse", Point::new(3.25, 4.0), &font, 1.5)
        .unwrap();
    let generation = first.atlas_upload.unwrap().generation;
    let second = text
        .shape_scaled("Reuse", Point::new(11.25, 14.0), &font, 1.5)
        .unwrap();

    assert!(second.atlas_upload.is_none());
    assert_eq!(text.atlas_stats().generation, generation);
    assert!(text.atlas_len() <= text.atlas_stats().entry_capacity);
}

#[test]
fn atlas_repack_is_bounded_and_stable_after_warmup() {
    let mut text = TextSystem::new(AtlasConfig::new(128, 128, 4)).unwrap();
    let font = FontSpec::system_ui(16.0);
    let first = text.shape("AB", Point::default(), &font).unwrap();
    let first_generation = first.atlas_upload.unwrap().generation;
    let rasterized_after_first = text.rasterized_glyph_count();

    assert!(
        text.shape("AB", Point::default(), &font)
            .unwrap()
            .atlas_upload
            .is_none()
    );
    assert_eq!(text.rasterized_glyph_count(), rasterized_after_first);

    let replaced = text.shape("CDEF", Point::default(), &font).unwrap();
    assert!(replaced.atlas_upload.unwrap().generation > first_generation);
    assert_eq!(text.atlas_repacks(), 1);
    assert_eq!(text.atlas_stats().entry_capacity, 4);
    assert_eq!(text.atlas_stats().resident_bytes, 128 * 128);

    for _ in 0..50 {
        assert!(
            text.shape("CDEF", Point::default(), &font)
                .unwrap()
                .atlas_upload
                .is_none()
        );
    }
    assert_eq!(text.atlas_len(), 4);
    assert_eq!(text.atlas_repacks(), 1);
}

#[test]
fn fractional_positions_select_distinct_masks_with_pixel_aligned_quads() {
    let mut text = TextSystem::new(AtlasConfig::new(128, 128, 32)).unwrap();
    let font = FontSpec::system_ui(18.0);
    let whole = text
        .shape_scaled("A", Point::new(0.0, 0.0), &font, 1.0)
        .unwrap();
    let fractional = text
        .shape_scaled("A", Point::new(0.5, 0.5), &font, 1.0)
        .unwrap();

    assert_ne!(whole.glyphs[0].uv_bounds, fractional.glyphs[0].uv_bounds);
    assert_eq!(text.atlas_len(), 2);
    for glyph in [whole.glyphs[0], fractional.glyphs[0]] {
        for edge in [
            glyph.bounds.origin.x,
            glyph.bounds.origin.y,
            glyph.bounds.size.width,
            glyph.bounds.size.height,
        ] {
            assert!((edge - edge.round()).abs() < 0.001);
        }
    }
}

#[test]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn atlas_masks_keep_a_transparent_safety_border() {
    let atlas_size = 128_u32;
    let mut text = TextSystem::new(AtlasConfig::new(atlas_size, atlas_size, 32)).unwrap();
    let shaped = text
        .shape("国MW", Point::default(), &FontSpec::system_ui(18.0))
        .unwrap();
    let upload = shaped.atlas_upload.unwrap();

    for glyph in shaped.glyphs.iter() {
        let x = (glyph.uv_bounds.origin.x * atlas_size as f32).round() as u32;
        let y = (glyph.uv_bounds.origin.y * atlas_size as f32).round() as u32;
        let width = (glyph.uv_bounds.size.width * atlas_size as f32).round() as u32;
        let height = (glyph.uv_bounds.size.height * atlas_size as f32).round() as u32;
        assert!(width >= 3 && height >= 3);
        for column in x..x + width {
            assert_eq!(upload.pixels[((y * atlas_size) + column) as usize], 0);
            assert_eq!(
                upload.pixels[(((y + height - 1) * atlas_size) + column) as usize],
                0
            );
        }
        for row in y..y + height {
            assert_eq!(upload.pixels[((row * atlas_size) + x) as usize], 0);
            assert_eq!(
                upload.pixels[((row * atlas_size) + x + width - 1) as usize],
                0
            );
        }
    }
}

#[test]
fn directwrite_rejects_invalid_capacities_sizes_and_scales() {
    assert_eq!(
        TextSystem::new(AtlasConfig::new(0, 64, 4)).unwrap_err(),
        TextError::InvalidAtlasDimensions
    );
    assert_eq!(
        TextSystem::new(AtlasConfig::new(64, 64, 0)).unwrap_err(),
        TextError::InvalidAtlasCapacity
    );

    let mut text = TextSystem::new(AtlasConfig::new(64, 64, 8)).unwrap();
    assert_eq!(
        text.shape_scaled("invalid", Point::default(), &FontSpec::system_ui(16.0), 0.0,)
            .unwrap_err(),
        TextError::InvalidFontSize
    );
}

#[test]
fn unknown_named_font_is_a_structured_error() {
    let mut text = TextSystem::new(AtlasConfig::new(128, 128, 32)).unwrap();
    let family = "Anmixiu Font That Cannot Exist 6B6F18F3";
    assert_eq!(
        text.shape("x", Point::default(), &FontSpec::new(family, 16.0))
            .unwrap_err(),
        TextError::FontUnavailable {
            family: family.to_owned()
        }
    );
}
