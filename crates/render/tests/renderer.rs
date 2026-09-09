use anmixiu_render::{Renderer, RendererConfig, SurfaceSize};
use std::sync::Arc;

use anmixiu_scene::{
    AtlasId, AtlasUpload, Clip, Color, DrawCommand, Glyph, PixelSize, Point, Rect, Scene, Size,
};

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::new(Point::new(x, y), Size::new(width, height))
}

#[test]
fn offscreen_srgb_color_preserves_encoded_components() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let scene = Scene::new(
        vec![DrawCommand::SolidQuad {
            bounds: rect(0.0, 0.0, 4.0, 4.0),
            color: Color::rgba(0.25, 0.5, 0.75, 1.0),
            clip: None,
        }],
        Vec::new(),
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(4, 4).expect("valid size"))
        .expect("render scene");

    for (actual, expected) in image
        .pixel_rgba(2, 2)
        .into_iter()
        .zip([64_u8, 128, 191, 255])
    {
        assert!(
            actual.abs_diff(expected) <= 1,
            "expected {expected}, got {actual}"
        );
    }
}

#[test]
fn rounded_quad_and_clip_reject_pixels_outside_the_shape() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let scene = Scene::new(
        vec![DrawCommand::RoundedQuad {
            bounds: rect(0.0, 0.0, 16.0, 16.0),
            color: Color::WHITE,
            corner_radius: 8.0,
            clip: Some(Clip::rectangular(rect(4.0, 0.0, 12.0, 16.0))),
        }],
        Vec::new(),
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(16, 16).expect("valid size"))
        .expect("render scene");

    assert_eq!(image.pixel_rgba(1, 8), [0, 0, 0, 0]);
    assert_eq!(image.pixel_rgba(8, 8), [255, 255, 255, 255]);
    assert_eq!(image.pixel_rgba(4, 0), [0, 0, 0, 0]);
}

#[test]
fn rounded_border_preserves_outer_and_inner_edges() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let scene = Scene::new(
        vec![DrawCommand::RoundedBorder {
            bounds: rect(0.0, 0.0, 16.0, 16.0),
            color: Color::WHITE,
            corner_radius: 8.0,
            border_width: 2.0,
            clip: None,
        }],
        Vec::new(),
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(16, 16).expect("valid size"))
        .expect("render scene");

    assert_eq!(image.pixel_rgba(0, 0), [0, 0, 0, 0]);
    assert_eq!(image.pixel_rgba(8, 0), [255, 255, 255, 255]);
    assert_eq!(image.pixel_rgba(8, 8), [0, 0, 0, 0]);
}

#[test]
fn glyphs_sample_the_uploaded_r8_atlas() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let atlas = AtlasId(7);
    let upload = AtlasUpload::new(atlas, 1, PixelSize::new(2, 2), Arc::from([255_u8; 4]))
        .expect("valid atlas");
    let scene = Scene::new(
        vec![DrawCommand::Glyphs {
            glyphs: Arc::from([Glyph::new(
                rect(0.0, 0.0, 4.0, 4.0),
                rect(0.0, 0.0, 1.0, 1.0),
                atlas,
            )]),
            color: Color::rgba(0.25, 0.5, 0.75, 1.0),
            clip: None,
        }],
        vec![upload],
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(4, 4).expect("valid size"))
        .expect("render glyph");

    let pixel = image.pixel_rgba(2, 2);
    for (actual, expected) in pixel.into_iter().zip([64_u8, 128, 191, 255]) {
        assert!(
            actual.abs_diff(expected) <= 1,
            "expected {expected}, got {actual}"
        );
    }
}

#[test]
fn logical_geometry_is_scaled_to_the_physical_target() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let scene = Scene::new(
        vec![DrawCommand::SolidQuad {
            bounds: rect(0.0, 0.0, 4.0, 4.0),
            color: Color::WHITE,
            clip: None,
        }],
        Vec::new(),
        Vec::new(),
    );

    let image = renderer
        .render_offscreen_scaled(&scene, SurfaceSize::new(12, 8).expect("valid size"), 2.0)
        .expect("render scaled scene");

    assert_eq!(image.pixel_rgba(7, 3), [255, 255, 255, 255]);
    assert_eq!(image.pixel_rgba(8, 3), [0, 0, 0, 0]);
}

#[test]
fn atlas_cache_is_generation_aware_and_hard_bounded() {
    let mut renderer = Renderer::with_config(RendererConfig {
        atlas_texture_capacity: 1,
    })
    .expect("wgpu renderer");
    let upload = |id, generation| {
        AtlasUpload::new(
            AtlasId(id),
            generation,
            PixelSize::new(2, 2),
            Arc::from([255_u8; 4]),
        )
        .expect("valid atlas")
    };
    let size = SurfaceSize::new(2, 2).expect("valid size");

    renderer
        .render_offscreen(
            &Scene::new(Vec::new(), vec![upload(10, 1)], Vec::new()),
            size,
        )
        .expect("first upload");
    renderer
        .render_offscreen(
            &Scene::new(Vec::new(), vec![upload(10, 1)], Vec::new()),
            size,
        )
        .expect("reuse upload");
    assert_eq!(renderer.stats().atlas_uploads, 1);

    renderer
        .render_offscreen(
            &Scene::new(Vec::new(), vec![upload(11, 1)], Vec::new()),
            size,
        )
        .expect("replacement upload");
    assert_eq!(renderer.stats().cached_atlases, 1);
    assert_eq!(renderer.stats().cached_atlas_bytes, 4);
    assert_eq!(renderer.stats().atlas_uploads, 2);
    assert_eq!(renderer.stats().atlas_evictions, 1);
}

#[test]
fn backdrop_blur_mixes_preceding_pixels_only_inside_its_bounds() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let scene = Scene::new(
        vec![
            DrawCommand::SolidQuad {
                bounds: rect(0.0, 0.0, 16.0, 16.0),
                color: Color::rgba(1.0, 0.0, 0.0, 1.0),
                clip: None,
            },
            DrawCommand::SolidQuad {
                bounds: rect(16.0, 0.0, 16.0, 16.0),
                color: Color::rgba(0.0, 0.0, 1.0, 1.0),
                clip: None,
            },
            DrawCommand::BackdropBlur {
                bounds: rect(8.0, 0.0, 16.0, 16.0),
                sigma: 3.0,
                corner_radius: 6.0,
                clip: Some(Clip::rectangular(rect(8.0, 0.0, 8.0, 16.0))),
            },
            DrawCommand::SolidQuad {
                bounds: rect(14.0, 6.0, 4.0, 4.0),
                color: Color::rgba(0.0, 1.0, 0.0, 1.0),
                clip: None,
            },
        ],
        Vec::new(),
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(32, 16).expect("valid size"))
        .expect("render backdrop blur");

    assert_eq!(image.pixel_rgba(4, 8), [255, 0, 0, 255]);
    assert_eq!(image.pixel_rgba(8, 0), [255, 0, 0, 255]);
    let mixed = image.pixel_rgba(15, 3);
    assert!(mixed[0] > 0 && mixed[2] > 0, "blurred boundary: {mixed:?}");
    assert_eq!(image.pixel_rgba(20, 3), [0, 0, 255, 255]);
    assert_eq!(image.pixel_rgba(15, 8), [0, 255, 0, 255]);
}

#[test]
fn filter_blur_affects_only_its_own_transparent_layer() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let scene = Scene::new(
        vec![
            DrawCommand::SolidQuad {
                bounds: rect(0.0, 0.0, 16.0, 16.0),
                color: Color::rgba(1.0, 0.0, 0.0, 1.0),
                clip: None,
            },
            DrawCommand::SolidQuad {
                bounds: rect(16.0, 0.0, 16.0, 16.0),
                color: Color::rgba(0.0, 0.0, 1.0, 1.0),
                clip: None,
            },
            DrawCommand::FilterBlur {
                sigma: 2.0,
                clip: None,
                commands: Arc::from([DrawCommand::SolidQuad {
                    bounds: rect(2.0, 8.0, 6.0, 6.0),
                    color: Color::rgba(0.0, 1.0, 0.0, 1.0),
                    clip: None,
                }]),
            },
        ],
        Vec::new(),
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(32, 16).expect("valid size"))
        .expect("render filter blur");

    assert_eq!(image.pixel_rgba(15, 2), [255, 0, 0, 255]);
    assert_eq!(image.pixel_rgba(16, 2), [0, 0, 255, 255]);
    let spread = image.pixel_rgba(1, 11);
    assert!(
        spread[0] > 0 && spread[1] > 0,
        "filtered spread: {spread:?}"
    );
    assert_eq!(spread[2], 0);
    assert_eq!(spread[3], 255);
    assert_eq!(image.pixel_rgba(24, 12), [0, 0, 255, 255]);
    assert_eq!(renderer.stats().filter_blur_operations, 1);
    assert_eq!(renderer.stats().compositor_texture_bytes, 32 * 16 * 4 * 4);
}

#[test]
fn blur_limits_are_rejected_before_compositor_allocation() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let backdrop = || DrawCommand::BackdropBlur {
        bounds: rect(0.0, 0.0, 8.0, 8.0),
        sigma: 1.0,
        corner_radius: 0.0,
        clip: None,
    };
    let too_many = Scene::new(
        (0..65).map(|_| backdrop()).collect(),
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(
        renderer
            .render_offscreen(&too_many, SurfaceSize::new(8, 8).expect("valid size"))
            .expect_err("effect count is bounded"),
        anmixiu_render::RenderError::TooManyBackdropBlurs
    );
    assert_eq!(renderer.stats().compositor_texture_bytes, 0);

    let mut nested = DrawCommand::SolidQuad {
        bounds: rect(0.0, 0.0, 1.0, 1.0),
        color: Color::WHITE,
        clip: None,
    };
    for _ in 0..9 {
        nested = DrawCommand::FilterBlur {
            sigma: 1.0,
            clip: None,
            commands: Arc::from([nested]),
        };
    }
    let too_deep = Scene::new(vec![nested], Vec::new(), Vec::new());
    assert_eq!(
        renderer
            .render_offscreen(&too_deep, SurfaceSize::new(8, 8).expect("valid size"))
            .expect_err("filter nesting is bounded"),
        anmixiu_render::RenderError::FilterBlurNestingTooDeep
    );
}

#[test]
fn backdrop_blur_averages_srgb_backdrops_in_linear_light() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let mut commands = (0_u16..64)
        .map(|x| DrawCommand::SolidQuad {
            bounds: rect(f32::from(x), 0.0, 1.0, 16.0),
            color: if x % 2 == 0 {
                Color::BLACK
            } else {
                Color::WHITE
            },
            clip: None,
        })
        .collect::<Vec<_>>();
    commands.push(DrawCommand::BackdropBlur {
        bounds: rect(0.0, 0.0, 64.0, 16.0),
        sigma: 3.0,
        corner_radius: 0.0,
        clip: None,
    });

    let image = renderer
        .render_offscreen(
            &Scene::new(commands, Vec::new(), Vec::new()),
            SurfaceSize::new(64, 16).expect("valid size"),
        )
        .expect("render blur");
    let pixel = image.pixel_rgba(32, 8);
    assert!(
        (180..=195).contains(&pixel[0]),
        "equal black/white energy should encode near sRGB 0.735: {pixel:?}"
    );
    assert_eq!(pixel[0], pixel[1]);
    assert_eq!(pixel[1], pixel[2]);
    assert_eq!(pixel[3], 255);
}

#[test]
fn nested_filter_blurs_use_distinct_bounded_layers() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let nested = DrawCommand::FilterBlur {
        sigma: 1.0,
        clip: None,
        commands: Arc::from([DrawCommand::FilterBlur {
            sigma: 1.0,
            clip: None,
            commands: Arc::from([DrawCommand::SolidQuad {
                bounds: rect(6.0, 6.0, 4.0, 4.0),
                color: Color::WHITE,
                clip: None,
            }]),
        }]),
    };
    let image = renderer
        .render_offscreen(
            &Scene::new(vec![nested], Vec::new(), Vec::new()),
            SurfaceSize::new(16, 16).expect("valid size"),
        )
        .expect("render nested filters");

    assert!(image.pixel_rgba(8, 8)[0] > 0);
    assert_eq!(renderer.stats().filter_blur_operations, 2);
    assert_eq!(renderer.stats().compositor_texture_bytes, 16 * 16 * 4 * 5);
}

#[test]
fn offscreen_solid_quad_can_be_read_back() {
    let mut renderer = Renderer::new().expect("wgpu renderer");
    let scene = Scene::new(
        vec![DrawCommand::SolidQuad {
            bounds: rect(0.0, 0.0, 8.0, 8.0),
            color: Color::rgba(1.0, 0.0, 0.0, 1.0),
            clip: None,
        }],
        Vec::new(),
        Vec::new(),
    );

    let image = renderer
        .render_offscreen(&scene, SurfaceSize::new(8, 8).expect("valid size"))
        .expect("render scene");

    assert_eq!(image.pixel_rgba(4, 4), [255, 0, 0, 255]);
    assert_eq!(renderer.stats().submitted_frames, 1);
}
