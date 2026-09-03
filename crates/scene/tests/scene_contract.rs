use std::{num::NonZeroUsize, sync::Arc};

use anmixiu_scene::{
    AtlasId, AtlasUpload, Clip, Color, DrawCommand, Glyph, HitId, HitRegion, PixelSize, Point,
    Rect, Scene, SceneCache, SceneCacheKey, Size,
};

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::new(Point::new(x, y), Size::new(width, height))
}

#[test]
fn geometry_uses_half_open_bounds_and_intersection() {
    let bounds = rect(10.0, 20.0, 30.0, 40.0);

    assert!(bounds.contains(Point::new(10.0, 20.0)));
    assert!(bounds.contains(Point::new(39.999, 59.999)));
    assert!(!bounds.contains(Point::new(40.0, 60.0)));
    assert_eq!(
        bounds.intersection(rect(20.0, 10.0, 30.0, 30.0)),
        Some(rect(20.0, 20.0, 20.0, 20.0))
    );
    assert_eq!(bounds.intersection(rect(40.0, 20.0, 1.0, 1.0)), None);
}

#[test]
fn colors_are_finite_and_clamped() {
    assert_eq!(
        Color::rgba(-1.0, 0.25, 2.0, f32::NAN),
        Color::rgba(0.0, 0.25, 1.0, 0.0)
    );
    assert!((Color::rgb(0.1, 0.2, 0.3).a - 1.0).abs() < f32::EPSILON);
}

#[test]
fn scene_preserves_platform_neutral_quad_glyph_and_upload_data() {
    assert_eq!(AtlasId::TEXT, AtlasId(1));
    let upload = AtlasUpload::new(
        AtlasId(7),
        3,
        PixelSize::new(2, 2),
        Arc::from([0_u8, 64, 128, 255]),
    )
    .expect("matching R8 byte count");
    let clip = Clip::rounded(rect(0.0, 0.0, 100.0, 100.0), 8.0);
    let commands = vec![
        DrawCommand::SolidQuad {
            bounds: rect(0.0, 0.0, 100.0, 100.0),
            color: Color::rgb(0.1, 0.2, 0.3),
            clip: None,
        },
        DrawCommand::RoundedQuad {
            bounds: rect(5.0, 5.0, 20.0, 20.0),
            color: Color::WHITE,
            corner_radius: 4.0,
            clip: Some(clip),
        },
        DrawCommand::RoundedBorder {
            bounds: rect(4.0, 4.0, 22.0, 22.0),
            color: Color::WHITE,
            corner_radius: 5.0,
            border_width: 1.0,
            clip: Some(clip),
        },
        DrawCommand::BackdropBlur {
            bounds: rect(6.0, 6.0, 18.0, 18.0),
            sigma: 8.0,
            corner_radius: 3.0,
            clip: Some(clip),
        },
        DrawCommand::Glyphs {
            glyphs: Arc::from([Glyph::new(
                rect(12.0, 13.0, 8.0, 10.0),
                rect(0.0, 0.0, 0.5, 0.5),
                AtlasId(7),
            )]),
            color: Color::BLACK,
            clip: Some(clip),
        },
    ];
    let scene = Scene::new(commands.clone(), vec![upload.clone()], Vec::new());

    assert_eq!(scene.commands(), commands.as_slice());
    assert_eq!(scene.atlas_uploads(), &[upload]);
}

#[test]
fn scene_reports_whether_ordered_backdrop_effects_require_compositing() {
    let plain = Scene::new(
        vec![DrawCommand::SolidQuad {
            bounds: rect(0.0, 0.0, 10.0, 10.0),
            color: Color::WHITE,
            clip: None,
        }],
        Vec::new(),
        Vec::new(),
    );
    let blurred = Scene::new(
        vec![DrawCommand::BackdropBlur {
            bounds: rect(1.0, 1.0, 8.0, 8.0),
            sigma: 4.0,
            corner_radius: 2.0,
            clip: None,
        }],
        Vec::new(),
        Vec::new(),
    );
    let identical_plain = plain.clone();

    assert!(!plain.requires_compositing());
    assert_eq!(
        plain, identical_plain,
        "lazy effect metadata is not scene data"
    );
    assert!(blurred.requires_compositing());
}

#[test]
fn invalid_atlas_upload_is_rejected() {
    let error = AtlasUpload::new(AtlasId(1), 1, PixelSize::new(3, 2), Arc::from([0_u8; 5]))
        .expect_err("R8 page must have width * height bytes");

    assert_eq!(error.expected_bytes(), 6);
    assert_eq!(error.actual_bytes(), 5);
}

#[test]
fn hit_testing_is_front_to_back_and_honors_rounded_clip() {
    let outer = rect(0.0, 0.0, 100.0, 100.0);
    let scene = Scene::new(
        Vec::new(),
        Vec::new(),
        vec![
            HitRegion::new(HitId(1), outer, None),
            HitRegion::new(HitId(2), outer, Some(Clip::rounded(outer, 20.0))),
        ],
    );

    assert_eq!(scene.hit_test(Point::new(50.0, 50.0)), Some(HitId(2)));
    // The rounded foreground excludes its extreme corner, revealing the region below it.
    assert_eq!(scene.hit_test(Point::new(1.0, 1.0)), Some(HitId(1)));
    assert_eq!(scene.hit_test(Point::new(101.0, 50.0)), None);
}

#[test]
fn scene_cache_reuses_entries_and_evicts_the_lru_at_its_hard_capacity() {
    let mut cache = SceneCache::new(NonZeroUsize::new(2).unwrap());
    let key_a = SceneCacheKey::new(1, 1, 1, 2.0);
    let key_b = SceneCacheKey::new(2, 1, 1, 2.0);
    let key_c = SceneCacheKey::new(3, 1, 1, 2.0);

    let a1 = cache.get_or_insert_with(key_a, Scene::empty);
    let b1 = cache.get_or_insert_with(key_b, Scene::empty);
    let a2 = cache.get_or_insert_with(key_a, || panic!("cache hit must not rebuild"));
    assert!(Arc::ptr_eq(&a1, &a2));

    cache.get_or_insert_with(key_c, Scene::empty);
    assert!(cache.contains(key_a));
    assert!(!cache.contains(key_b));
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 3);
    assert_eq!(cache.stats().evictions, 1);
    assert!(
        Arc::strong_count(&b1) >= 1,
        "external reuse remains valid after eviction"
    );
}

#[test]
fn cache_key_isolated_by_paint_layout_and_scale_revisions() {
    let base = SceneCacheKey::new(9, 4, 8, 2.0);

    assert_ne!(base, SceneCacheKey::new(9, 5, 8, 2.0));
    assert_ne!(base, SceneCacheKey::new(9, 4, 9, 2.0));
    assert_ne!(base, SceneCacheKey::new(9, 4, 8, 1.0));
}

#[test]
fn explicit_invalidation_drops_only_the_selected_entry() {
    let mut cache = SceneCache::new(NonZeroUsize::new(2).unwrap());
    let key_a = SceneCacheKey::new(1, 1, 1, 1.0);
    let key_b = SceneCacheKey::new(2, 1, 1, 1.0);
    cache.get_or_insert_with(key_a, Scene::empty);
    cache.get_or_insert_with(key_b, Scene::empty);

    assert!(cache.invalidate(key_a));
    assert!(!cache.contains(key_a));
    assert!(cache.contains(key_b));
}
