#![cfg(target_os = "macos")]

use std::sync::Arc;

use anmixiu_render_metal::{FrameOutcome, MetalRenderer, RendererConfig, SurfaceSize};
use anmixiu_scene::{
    AtlasId, AtlasUpload, Clip, Color, DrawCommand, PixelSize, Point, Rect, Scene, Size,
};
use anmixiu_text_coretext::{AtlasConfig, FontSpec, TextSystem};
use core_graphics::geometry::CGSize;
use metal::MetalLayer;

fn scene_with(command: DrawCommand) -> Scene {
    Scene::new(vec![command], Vec::new(), Vec::new())
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::new(Point::new(x, y), Size::new(width, height))
}

#[test]
fn offscreen_solid_quad_can_be_read_back() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let scene = scene_with(DrawCommand::SolidQuad {
        bounds: rect(0.0, 0.0, 8.0, 8.0),
        color: Color::rgba(1.0, 0.0, 0.0, 1.0),
        clip: None,
    });

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(8, 8).unwrap())
        .unwrap();

    assert_eq!(image.pixel_rgba(4, 4), [255, 0, 0, 255]);
    assert_eq!(renderer.stats().submitted_frames, 1);
}

#[test]
fn offscreen_rounding_and_clip_reject_pixels_outside_the_shape() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let scene = scene_with(DrawCommand::RoundedQuad {
        bounds: rect(0.0, 0.0, 16.0, 16.0),
        color: Color::WHITE,
        corner_radius: 8.0,
        clip: Some(Clip {
            bounds: rect(4.0, 0.0, 12.0, 16.0),
            corner_radius: 0.0,
        }),
    });

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(16, 16).unwrap())
        .unwrap();

    assert_eq!(image.pixel_rgba(1, 8), [0, 0, 0, 0], "clipped pixel");
    assert_eq!(image.pixel_rgba(8, 8), [255, 255, 255, 255]);
    assert_eq!(image.pixel_rgba(4, 0), [0, 0, 0, 0], "rounded corner");
}

#[test]
fn offscreen_rounded_border_preserves_rounded_outer_and_inner_edges() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let scene = scene_with(DrawCommand::RoundedBorder {
        bounds: rect(0.0, 0.0, 16.0, 16.0),
        color: Color::WHITE,
        corner_radius: 8.0,
        border_width: 2.0,
        clip: None,
    });

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(16, 16).unwrap())
        .unwrap();

    assert_eq!(image.pixel_rgba(0, 0), [0, 0, 0, 0], "rounded outer edge");
    assert_eq!(image.pixel_rgba(8, 0), [255, 255, 255, 255], "border");
    assert_eq!(image.pixel_rgba(8, 8), [0, 0, 0, 0], "hollow center");
}

#[test]
fn unavailable_drawable_does_not_submit_or_request_a_busy_retry() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let before = renderer.stats();

    let outcome = renderer
        .render_optional_drawable(None, &Scene::empty())
        .unwrap();

    assert_eq!(
        outcome,
        FrameOutcome::DrawableUnavailable {
            retry_immediately: false
        }
    );
    assert_eq!(renderer.stats().submitted_frames, before.submitted_frames);
    assert_eq!(renderer.stats().drawable_misses, before.drawable_misses + 1);
}

#[test]
fn invalid_surface_size_is_rejected_before_metal_submission() {
    let error = SurfaceSize::new(0, 10).unwrap_err();
    assert!(error.to_string().contains("non-zero"));
}

#[test]
fn configured_surface_rejects_stale_cross_display_drawable_dimensions() {
    let expected = SurfaceSize::new(620, 520).unwrap();
    let stale_retina = SurfaceSize::new(1240, 1040).unwrap();

    assert!(expected.matches(stale_retina).is_err());
    assert!(expected.matches(expected).is_ok());
}

#[test]
fn core_text_english_and_chinese_atlas_renders_visible_pixels() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
    let shaped = text
        .shape(
            "Hello 你好",
            Point::new(4.0, 4.0),
            &FontSpec::new("Helvetica", 24.0),
        )
        .unwrap();
    let scene = Scene::new(
        vec![DrawCommand::Glyphs {
            glyphs: shaped.glyphs,
            color: Color::WHITE,
            clip: None,
        }],
        vec![shaped.atlas_upload.unwrap()],
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(160, 40).unwrap())
        .unwrap();

    let visible_pixels = image
        .pixels()
        .chunks(4)
        .filter(|pixel| pixel[3] != 0)
        .count();
    assert!(
        visible_pixels > 100,
        "glyph shader should sample the R8 atlas"
    );
}

#[test]
fn core_text_atlas_rows_are_top_down_in_the_metal_scene() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let mut text = TextSystem::new(AtlasConfig::new(128, 128, 32)).unwrap();
    let shaped = text
        .shape("F", Point::new(4.0, 4.0), &FontSpec::new("Helvetica", 30.0))
        .unwrap();
    let bounds = shaped.glyphs[0].bounds;
    let scene = Scene::new(
        vec![DrawCommand::Glyphs {
            glyphs: shaped.glyphs,
            color: Color::WHITE,
            clip: None,
        }],
        vec![shaped.atlas_upload.unwrap()],
        Vec::new(),
    );
    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(48, 48).unwrap())
        .unwrap();
    let split = bounds.origin.y + bounds.size.height / 2.0;
    let mut top_alpha = 0_u64;
    let mut bottom_alpha = 0_u64;
    for y in 0_u16..48 {
        for x in 0_u16..48 {
            let point = Point::new(f32::from(x), f32::from(y));
            if !bounds.contains(point) {
                continue;
            }
            let alpha = u64::from(image.pixel_rgba(u32::from(x), u32::from(y))[3]);
            if f32::from(y) < split {
                top_alpha += alpha;
            } else {
                bottom_alpha += alpha;
            }
        }
    }
    assert!(
        top_alpha > bottom_alpha,
        "upright F has more ink in its top half; top={top_alpha}, bottom={bottom_alpha}"
    );
}

/// Renders a solid frame through `render_layer` against a configured off-screen `CAMetalLayer`,
/// retrying while the drawable pool is momentarily empty. Returns the presenting outcome, or the
/// last non-presenting outcome if the pool never yields a drawable on this host.
fn present_once(renderer: &mut MetalRenderer, layer: &MetalLayer) -> FrameOutcome {
    let scene = scene_with(DrawCommand::SolidQuad {
        bounds: rect(0.0, 0.0, 32.0, 24.0),
        color: Color::rgba(0.2, 0.4, 0.8, 1.0),
        clip: None,
    });
    let mut last = FrameOutcome::DrawableUnavailable {
        retry_immediately: false,
    };
    for _ in 0..16 {
        last = renderer.render_layer(layer, &scene, 1.0).unwrap();
        if last == FrameOutcome::Presented {
            break;
        }
    }
    last
}

#[test]
fn configure_layer_opts_into_transaction_present() {
    let Some(renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let layer = MetalLayer::new();
    renderer.configure_layer(&layer, SurfaceSize::new(64, 48).unwrap(), 1.0);
    assert!(
        layer.presents_with_transaction(),
        "live-resize sync requires transaction-coordinated presents"
    );
}

#[test]
fn render_layer_presents_through_the_transaction_path() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let layer = MetalLayer::new();
    layer.set_drawable_size(CGSize::new(64.0, 48.0));
    renderer.configure_layer(&layer, SurfaceSize::new(64, 48).unwrap(), 1.0);
    assert!(layer.presents_with_transaction());

    let before = renderer.stats().submitted_frames;
    let outcome = present_once(&mut renderer, &layer);
    // A headless host may never vend a drawable; only assert submission when one presented.
    if outcome == FrameOutcome::Presented {
        assert_eq!(renderer.stats().submitted_frames, before + 1);
    } else {
        eprintln!("layer never vended a drawable on this host: {outcome:?}");
    }
}

#[test]
fn render_layer_presents_when_transaction_is_disabled() {
    let Some(mut renderer) = MetalRenderer::new().unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let layer = MetalLayer::new();
    layer.set_drawable_size(CGSize::new(48.0, 32.0));
    renderer.configure_layer(&layer, SurfaceSize::new(48, 32).unwrap(), 1.0);
    // Force the non-transaction branch (GPU-registered present via the command buffer).
    layer.set_presents_with_transaction(false);

    let before = renderer.stats().submitted_frames;
    let outcome = present_once(&mut renderer, &layer);
    if outcome == FrameOutcome::Presented {
        assert_eq!(renderer.stats().submitted_frames, before + 1);
    } else {
        eprintln!("layer never vended a drawable on this host: {outcome:?}");
    }
}

#[test]
fn atlas_texture_cache_is_generation_aware_and_hard_bounded() {
    let Some(mut renderer) = MetalRenderer::with_config(RendererConfig {
        atlas_texture_capacity: 1,
    })
    .unwrap() else {
        eprintln!("Metal device unavailable on this macOS host");
        return;
    };
    let upload = |id, generation| {
        AtlasUpload::new(
            AtlasId(id),
            generation,
            PixelSize::new(2, 2),
            Arc::from([255_u8; 4]),
        )
        .unwrap()
    };
    let size = SurfaceSize::new(2, 2).unwrap();

    renderer
        .render_offscreen(
            &Scene::new(Vec::new(), vec![upload(10, 1)], Vec::new()),
            size,
        )
        .unwrap();
    renderer
        .render_offscreen(
            &Scene::new(Vec::new(), vec![upload(10, 1)], Vec::new()),
            size,
        )
        .unwrap();
    assert_eq!(
        renderer.stats().atlas_uploads,
        1,
        "same generation reuses texture"
    );

    renderer
        .render_offscreen(
            &Scene::new(Vec::new(), vec![upload(11, 1)], Vec::new()),
            size,
        )
        .unwrap();
    assert_eq!(renderer.stats().cached_atlases, 1);
    assert_eq!(renderer.stats().cached_atlas_bytes, 4);
    assert_eq!(renderer.stats().atlas_uploads, 2);
}
