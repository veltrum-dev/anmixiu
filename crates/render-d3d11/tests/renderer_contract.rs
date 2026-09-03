#![cfg_attr(target_os = "windows", allow(unsafe_code))]

use anmixiu_render_d3d11::{RenderError, SurfaceSize};

#[test]
fn surface_sizes_are_non_empty_and_match_exact_physical_pixels() {
    assert_eq!(
        SurfaceSize::new(0, 100),
        Err(RenderError::InvalidSurfaceSize {
            width: 0,
            height: 100
        })
    );
    let expected = SurfaceSize::new(1241, 1041).unwrap();
    assert_eq!(expected.matches(expected), Ok(()));
    assert_eq!(
        expected.matches(SurfaceSize::new(1240, 1040).unwrap()),
        Err(RenderError::SurfaceOutOfDate {
            expected,
            actual: SurfaceSize::new(1240, 1040).unwrap()
        })
    );
}

#[cfg(target_os = "windows")]
mod windows_rendering {
    use std::sync::Arc;

    use anmixiu_render_d3d11::{D3d11Renderer, FrameOutcome, RendererConfig, SurfaceSize};
    use anmixiu_scene::{AtlasId, AtlasUpload, Color, DrawCommand, PixelSize, Point, Scene};
    use anmixiu_text_directwrite::{AtlasConfig, FontSpec, TextSystem};
    use windows::{
        Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
            },
        },
        core::w,
    };

    struct TestWindow(HWND);

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: The test owns this hidden HWND and drops the renderer before the window.
            let _destroyed = unsafe { DestroyWindow(self.0) };
        }
    }

    fn hidden_window() -> TestWindow {
        // SAFETY: `STATIC` is a process-wide system class. This creates one hidden top-level
        // window without a parent, menu, instance payload, or creation pointer.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Anmixiu D3D11 test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                160,
                64,
                None,
                None,
                None,
                None,
            )
        }
        .expect("hidden test window should be available");
        TestWindow(hwnd)
    }

    #[test]
    fn directwrite_a8_atlas_can_be_uploaded_and_presented() {
        let window = hidden_window();
        let size = SurfaceSize::new(160, 64).unwrap();
        let mut renderer = D3d11Renderer::new(window.0, size, 1.0).unwrap();
        let mut text = TextSystem::new(AtlasConfig::new(256, 256, 128)).unwrap();
        let shaped = text
            .shape(
                "Hello 你好",
                Point::new(4.0, 4.0),
                &FontSpec::system_ui(24.0),
            )
            .unwrap();
        let scene = Scene::new(
            vec![DrawCommand::Glyphs {
                glyphs: shaped.glyphs,
                color: Color::WHITE,
                clip: None,
            }],
            vec![shaped.atlas_upload.expect("first shape uploads the atlas")],
            Vec::new(),
        );

        assert_eq!(
            renderer.render(&scene, size, 1.0).unwrap(),
            FrameOutcome::Presented
        );
        assert_eq!(renderer.stats().atlas_uploads, 1);
        assert_eq!(renderer.stats().presented_frames, 1);
        assert_eq!(renderer.stats().composited_frames, 0);
        assert_eq!(renderer.stats().compositor_texture_bytes, 0);
    }

    #[test]
    fn atlas_texture_cache_reuses_generations_and_enforces_capacity() {
        let window = hidden_window();
        let size = SurfaceSize::new(160, 64).unwrap();
        let mut renderer = D3d11Renderer::with_config(
            window.0,
            size,
            1.0,
            RendererConfig {
                atlas_texture_capacity: 1,
            },
        )
        .unwrap();
        let upload = |atlas, generation| {
            AtlasUpload::new(
                AtlasId(atlas),
                generation,
                PixelSize::new(2, 2),
                Arc::from([255_u8; 4]),
            )
            .unwrap()
        };

        for scene in [
            Scene::new(Vec::new(), vec![upload(10, 1)], Vec::new()),
            Scene::new(Vec::new(), vec![upload(10, 1)], Vec::new()),
            Scene::new(Vec::new(), vec![upload(11, 1)], Vec::new()),
        ] {
            assert_eq!(
                renderer.render(&scene, size, 1.0).unwrap(),
                FrameOutcome::Presented
            );
        }

        let stats = renderer.stats();
        assert_eq!(stats.atlas_uploads, 2);
        assert_eq!(stats.atlas_evictions, 1);
        assert_eq!(stats.cached_atlases, 1);
        assert_eq!(stats.cached_atlas_bytes, 4);
    }

    #[test]
    fn resized_surface_rejects_the_previous_physical_size_and_scale() {
        let window = hidden_window();
        let initial = SurfaceSize::new(160, 64).unwrap();
        let resized = SurfaceSize::new(240, 96).unwrap();
        let mut renderer = D3d11Renderer::new(window.0, initial, 1.0).unwrap();

        renderer.resize(resized, 1.5).unwrap();
        assert_eq!(renderer.surface_size(), resized);
        assert_eq!(
            renderer.render(&Scene::empty(), initial, 1.0).unwrap(),
            FrameOutcome::SurfaceOutOfDate {
                expected: resized,
                actual: initial,
                retry_immediately: true,
            }
        );
        assert_eq!(
            renderer.render(&Scene::empty(), resized, 1.5).unwrap(),
            FrameOutcome::Presented
        );
    }

    #[test]
    fn backdrop_blur_uses_the_bounded_intermediate_compositor() {
        let window = hidden_window();
        let size = SurfaceSize::new(160, 64).unwrap();
        let mut renderer = D3d11Renderer::new(window.0, size, 1.0).unwrap();
        let scene = Scene::new(
            vec![
                DrawCommand::SolidQuad {
                    bounds: anmixiu_scene::Rect::new(
                        Point::new(0.0, 0.0),
                        anmixiu_scene::Size::new(160.0, 64.0),
                    ),
                    color: Color::WHITE,
                    clip: None,
                },
                DrawCommand::BackdropBlur {
                    bounds: anmixiu_scene::Rect::new(
                        Point::new(40.0, 8.0),
                        anmixiu_scene::Size::new(80.0, 48.0),
                    ),
                    sigma: 8.0,
                    corner_radius: 12.0,
                    clip: None,
                },
            ],
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(
            renderer.render(&scene, size, 1.0).unwrap(),
            FrameOutcome::Presented
        );
        assert_eq!(renderer.stats().composited_frames, 1);
        assert_eq!(renderer.stats().backdrop_blur_operations, 1);
        assert!(renderer.stats().compositor_texture_bytes > 0);
    }
}
