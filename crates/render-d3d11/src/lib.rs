//! D3D11/DXGI renderer for immutable [`anmixiu_scene::Scene`] snapshots.
//!
//! Drawing is encoded through a Direct2D device context backed by the renderer's
//! D3D11 device and swap chain. R8 glyph atlases are retained in a hard-capacity
//! LRU keyed by `(AtlasId, generation)`; a newer generation replaces the older
//! texture for the same atlas id.

#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceSize {
    width: u32,
    height: u32,
}

impl SurfaceSize {
    /// Creates a non-empty physical surface size.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidSurfaceSize`] when either dimension is zero.
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        if width == 0 || height == 0 {
            Err(RenderError::InvalidSurfaceSize { width, height })
        } else {
            Ok(Self { width, height })
        }
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Verifies that a drawable has this exact physical size.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::SurfaceOutOfDate`] when the dimensions differ.
    pub fn matches(self, actual: Self) -> Result<(), RenderError> {
        if self == actual {
            Ok(())
        } else {
            Err(RenderError::SurfaceOutOfDate {
                expected: self,
                actual,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RendererConfig {
    pub atlas_texture_capacity: usize,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            atlas_texture_capacity: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderStats {
    pub presented_frames: u64,
    pub draw_calls: u64,
    pub atlas_uploads: u64,
    pub atlas_evictions: u64,
    pub cached_atlases: usize,
    pub cached_atlas_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameOutcome {
    Presented,
    SurfaceOutOfDate {
        expected: SurfaceSize,
        actual: SurfaceSize,
        retry_immediately: bool,
    },
}

#[derive(Debug, Error, PartialEq)]
pub enum RenderError {
    #[error("surface dimensions must be non-zero, got {width}x{height}")]
    InvalidSurfaceSize { width: u32, height: u32 },
    #[error("render scale must be finite and greater than zero")]
    InvalidScale,
    #[error("drawable surface is out of date: expected {expected:?}, got {actual:?}")]
    SurfaceOutOfDate {
        expected: SurfaceSize,
        actual: SurfaceSize,
    },
    #[error("atlas texture capacity must be non-zero")]
    InvalidAtlasCapacity,
    #[error("atlas {atlas} generation {generation} has {actual} bytes, expected {expected}")]
    InvalidAtlasUpload {
        atlas: u64,
        generation: u64,
        expected: usize,
        actual: usize,
    },
    #[error("scene references atlas {atlas}, but it has not been uploaded")]
    MissingAtlas { atlas: u64 },
    #[error("D3D11/Direct2D operation failed: {0}")]
    Graphics(String),
    #[error("D3D11 is only available on Windows")]
    UnsupportedPlatform,
}

#[cfg(target_os = "windows")]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
mod platform {
    use std::{
        collections::HashMap, marker::PhantomData, mem::ManuallyDrop, num::NonZeroUsize, rc::Rc,
    };

    use anmixiu_scene::{AtlasId, AtlasUpload, Clip, Color, DrawCommand, Rect, Scene};
    use windows::{
        Win32::{
            Foundation::{HMODULE, HWND},
            Graphics::{
                Direct2D::Common::{
                    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
                    D2D1_PIXEL_FORMAT,
                },
                Direct2D::{
                    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE,
                    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
                    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_LAYER_OPTIONS1_NONE,
                    D2D1_LAYER_PARAMETERS1, D2D1_ROUNDED_RECT, D2D1_UNIT_MODE_DIPS,
                    D2D1CreateDevice, ID2D1Bitmap1, ID2D1Brush, ID2D1Device, ID2D1DeviceContext,
                    ID2D1Factory, ID2D1Geometry, ID2D1Image, ID2D1SolidColorBrush,
                },
                Direct3D::{
                    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0,
                    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
                },
                Direct3D11::{
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
                    D3D11CreateDeviceAndSwapChain, ID3D11Device,
                },
                Dxgi::{
                    Common::{
                        DXGI_FORMAT_A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_DESC,
                        DXGI_MODE_SCALING_UNSPECIFIED, DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED,
                        DXGI_RATIONAL, DXGI_SAMPLE_DESC,
                    },
                    DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_CHAIN_FLAG,
                    DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice,
                    IDXGISurface, IDXGISwapChain,
                },
            },
        },
        core::{Interface, Result as WindowsResult},
    };
    use windows_numerics::Matrix3x2;

    use super::{FrameOutcome, RenderError, RenderStats, RendererConfig, SurfaceSize};

    #[allow(clippy::needless_pass_by_value)]
    fn graphics_error(error: windows::core::Error) -> RenderError {
        RenderError::Graphics(error.to_string())
    }

    #[derive(Debug)]
    struct CachedAtlas {
        generation: u64,
        bitmap: ID2D1Bitmap1,
        size: (u32, u32),
        bytes: usize,
        last_used: u64,
    }

    /// Main-thread D3D11 renderer. `Rc` phantom data prevents accidental transfer to a worker.
    pub struct D3d11Renderer {
        _d3d_device: ID3D11Device,
        _d2d_device: ID2D1Device,
        context: ID2D1DeviceContext,
        factory: ID2D1Factory,
        swap_chain: IDXGISwapChain,
        target: Option<ID2D1Bitmap1>,
        brush: ID2D1SolidColorBrush,
        surface: SurfaceSize,
        scale: f32,
        atlas_capacity: NonZeroUsize,
        atlases: HashMap<AtlasId, CachedAtlas>,
        atlas_clock: u64,
        stats: RenderStats,
        _main_thread: PhantomData<Rc<()>>,
    }

    impl D3d11Renderer {
        /// Creates a renderer for a live Win32 client surface.
        ///
        /// # Errors
        ///
        /// Returns an error when the scale or configuration is invalid, or when D3D11,
        /// DXGI, or Direct2D cannot initialize the requested surface.
        pub fn new(hwnd: HWND, size: SurfaceSize, scale: f32) -> Result<Self, RenderError> {
            Self::with_config(hwnd, size, scale, RendererConfig::default())
        }

        /// Creates a renderer with explicit bounded-cache configuration.
        ///
        /// # Errors
        ///
        /// Returns an error when the scale or atlas capacity is invalid, or when native
        /// graphics initialization fails.
        pub fn with_config(
            hwnd: HWND,
            size: SurfaceSize,
            scale: f32,
            config: RendererConfig,
        ) -> Result<Self, RenderError> {
            if !scale.is_finite() || scale <= 0.0 {
                return Err(RenderError::InvalidScale);
            }
            let atlas_capacity = NonZeroUsize::new(config.atlas_texture_capacity)
                .ok_or(RenderError::InvalidAtlasCapacity)?;
            let desc = DXGI_SWAP_CHAIN_DESC {
                BufferDesc: DXGI_MODE_DESC {
                    Width: size.width(),
                    Height: size.height(),
                    RefreshRate: DXGI_RATIONAL {
                        Numerator: 0,
                        Denominator: 1,
                    },
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    ScanlineOrdering: DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED,
                    Scaling: DXGI_MODE_SCALING_UNSPECIFIED,
                },
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                OutputWindow: hwnd,
                Windowed: true.into(),
                SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
                Flags: 0,
            };
            let levels = [
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_10_0,
            ];
            let (d3d_device, swap_chain) = create_device(&desc, &levels)?;
            let dxgi_device: IDXGIDevice = d3d_device.cast().map_err(graphics_error)?;
            // SAFETY: The DXGI device comes from the BGRA-capable D3D11 device created above;
            // Direct2D retains the COM interface for the lifetime of its device.
            let d2d_device =
                unsafe { D2D1CreateDevice(&dxgi_device, None) }.map_err(graphics_error)?;
            // SAFETY: Options request the standard immediate device context.
            let context =
                unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }
                    .map_err(graphics_error)?;
            // SAFETY: Every Direct2D resource exposes its creating factory.
            let factory = unsafe { context.GetFactory() }.map_err(graphics_error)?;
            let transparent = D2D1_COLOR_F::default();
            // SAFETY: The color pointer is valid for the synchronous brush creation call.
            let brush = unsafe { context.CreateSolidColorBrush(&raw const transparent, None) }
                .map_err(graphics_error)?;
            let target = create_target(&context, &swap_chain, scale)?;
            // SAFETY: The target bitmap was created from this swap chain on this context.
            unsafe {
                context.SetTarget(&target);
                context.SetUnitMode(D2D1_UNIT_MODE_DIPS);
                context.SetDpi(96.0 * scale, 96.0 * scale);
            }
            Ok(Self {
                _d3d_device: d3d_device,
                _d2d_device: d2d_device,
                context,
                factory,
                swap_chain,
                target: Some(target),
                brush,
                surface: size,
                scale,
                atlas_capacity,
                atlases: HashMap::with_capacity(atlas_capacity.get()),
                atlas_clock: 0,
                stats: RenderStats::default(),
                _main_thread: PhantomData,
            })
        }

        #[must_use]
        pub const fn surface_size(&self) -> SurfaceSize {
            self.surface
        }

        #[must_use]
        pub const fn stats(&self) -> RenderStats {
            self.stats
        }

        /// Recreates swap-chain resources for a new physical size or display scale.
        ///
        /// # Errors
        ///
        /// Returns an error when `scale` is invalid or DXGI/Direct2D resource recreation fails.
        pub fn resize(&mut self, size: SurfaceSize, scale: f32) -> Result<(), RenderError> {
            if !scale.is_finite() || scale <= 0.0 {
                return Err(RenderError::InvalidScale);
            }
            if self.surface == size && self.scale.to_bits() == scale.to_bits() {
                return Ok(());
            }
            // SAFETY: Unbinding and dropping every reference to the current back buffer is
            // required before DXGI ResizeBuffers; the context and swap chain remain live.
            unsafe { self.context.SetTarget(None::<&ID2D1Image>) };
            self.target = None;
            // SAFETY: Width and height are non-zero by SurfaceSize construction; format and flags
            // are preserved from the original swap chain.
            unsafe {
                self.swap_chain.ResizeBuffers(
                    0,
                    size.width(),
                    size.height(),
                    windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
            }
            .map_err(graphics_error)?;
            let target = create_target(&self.context, &self.swap_chain, scale)?;
            // SAFETY: The new target belongs to this context and current swap-chain buffer.
            unsafe {
                self.context.SetTarget(&target);
                self.context.SetDpi(96.0 * scale, 96.0 * scale);
            }
            self.target = Some(target);
            self.surface = size;
            self.scale = scale;
            Ok(())
        }

        /// Draws and presents one immutable scene snapshot.
        ///
        /// # Errors
        ///
        /// Returns an error for malformed or missing atlas data and for native drawing or
        /// presentation failures. A size/scale mismatch is reported as a non-error outcome so
        /// the platform host can synchronize the surface and retry on a later frame.
        pub fn render(
            &mut self,
            scene: &Scene,
            actual: SurfaceSize,
            scale: f32,
        ) -> Result<FrameOutcome, RenderError> {
            if self.surface != actual || self.scale.to_bits() != scale.to_bits() {
                return Ok(FrameOutcome::SurfaceOutOfDate {
                    expected: self.surface,
                    actual,
                    retry_immediately: true,
                });
            }
            self.upload_atlases(scene.atlas_uploads())?;
            self.validate_scene_atlases(scene)?;
            let clear = D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };
            // SAFETY: All drawing occurs on the creating UI thread between one balanced
            // BeginDraw/EndDraw pair; no CPU-writable D3D resource is reused in flight.
            unsafe {
                self.context.BeginDraw();
                self.context.Clear(Some(&raw const clear));
            }
            let draw_result = scene
                .commands()
                .iter()
                .try_for_each(|command| self.draw_command(command));
            // SAFETY: This balances BeginDraw even when command validation returns an error.
            let end_result = unsafe { self.context.EndDraw(None, None) }.map_err(graphics_error);
            draw_result?;
            end_result?;
            // SAFETY: Present queues the completed back buffer with vsync and returns without a
            // CPU wait for GPU completion.
            unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) }
                .ok()
                .map_err(graphics_error)?;
            self.stats.presented_frames = self.stats.presented_frames.saturating_add(1);
            self.refresh_cache_stats();
            Ok(FrameOutcome::Presented)
        }

        fn draw_command(&mut self, command: &DrawCommand) -> Result<(), RenderError> {
            match command {
                DrawCommand::SolidQuad {
                    bounds,
                    color,
                    clip,
                } => self.draw_with_clip(*clip, |renderer| {
                    renderer.set_brush(*color);
                    let rect = d2d_rect(*bounds);
                    // SAFETY: Rect and brush are valid for this synchronous draw call.
                    unsafe {
                        renderer
                            .context
                            .FillRectangle(&raw const rect, &renderer.brush);
                    }
                    Ok(1)
                }),
                DrawCommand::RoundedQuad {
                    bounds,
                    color,
                    corner_radius,
                    clip,
                } => self.draw_with_clip(*clip, |renderer| {
                    renderer.set_brush(*color);
                    let rounded = D2D1_ROUNDED_RECT {
                        rect: d2d_rect(*bounds),
                        radiusX: *corner_radius,
                        radiusY: *corner_radius,
                    };
                    // SAFETY: Rounded rectangle and brush remain valid through the draw call.
                    unsafe {
                        renderer
                            .context
                            .FillRoundedRectangle(&raw const rounded, &renderer.brush);
                    };
                    Ok(1)
                }),
                DrawCommand::RoundedBorder {
                    bounds,
                    color,
                    corner_radius,
                    border_width,
                    clip,
                } => self.draw_with_clip(*clip, |renderer| {
                    let border_width = border_width
                        .max(0.0)
                        .min(bounds.size.width.max(0.0) / 2.0)
                        .min(bounds.size.height.max(0.0) / 2.0);
                    if border_width <= 0.0 {
                        return Ok(0);
                    }
                    renderer.set_brush(*color);
                    let half_width = border_width / 2.0;
                    let mut rect = d2d_rect(*bounds);
                    rect.left += half_width;
                    rect.top += half_width;
                    rect.right -= half_width;
                    rect.bottom -= half_width;
                    let radius = (corner_radius.max(0.0) - half_width).max(0.0);
                    let rounded = D2D1_ROUNDED_RECT {
                        rect,
                        radiusX: radius,
                        radiusY: radius,
                    };
                    // SAFETY: Rounded rectangle and brush remain valid through the draw call;
                    // the absent stroke style requests Direct2D's solid default stroke.
                    unsafe {
                        renderer.context.DrawRoundedRectangle(
                            &raw const rounded,
                            &renderer.brush,
                            border_width,
                            None,
                        );
                    };
                    Ok(1)
                }),
                DrawCommand::Glyphs {
                    glyphs,
                    color,
                    clip,
                } => self.draw_with_clip(*clip, |renderer| {
                    renderer.set_brush(*color);
                    // Direct2D requires aliased primitive AA for opacity masks; the atlas itself
                    // already contains DirectWrite antialias coverage.
                    // SAFETY: Context state changes and mask draws are confined to this frame.
                    unsafe {
                        renderer
                            .context
                            .SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
                    };
                    for glyph in glyphs.iter() {
                        let atlas = renderer.atlases.get_mut(&glyph.atlas).ok_or(
                            RenderError::MissingAtlas {
                                atlas: glyph.atlas.0,
                            },
                        )?;
                        renderer.atlas_clock = renderer.atlas_clock.saturating_add(1);
                        atlas.last_used = renderer.atlas_clock;
                        let destination = d2d_rect(glyph.bounds);
                        let source = D2D_RECT_F {
                            left: glyph.uv_bounds.origin.x * atlas.size.0 as f32,
                            top: glyph.uv_bounds.origin.y * atlas.size.1 as f32,
                            right: glyph.uv_bounds.max_x() * atlas.size.0 as f32,
                            bottom: glyph.uv_bounds.max_y() * atlas.size.1 as f32,
                        };
                        // SAFETY: Atlas bitmap, source rectangle, destination rectangle and brush
                        // all remain live for this synchronous opacity-mask draw.
                        unsafe {
                            renderer.context.FillOpacityMask(
                                &atlas.bitmap,
                                &renderer.brush,
                                Some(&raw const destination),
                                Some(&raw const source),
                            );
                        };
                    }
                    // SAFETY: Restore the renderer's normal geometry antialias mode.
                    unsafe {
                        renderer
                            .context
                            .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    };
                    Ok(u64::try_from(glyphs.len()).unwrap_or(u64::MAX))
                }),
            }
        }

        fn draw_with_clip(
            &mut self,
            clip: Option<Clip>,
            draw: impl FnOnce(&mut Self) -> Result<u64, RenderError>,
        ) -> Result<(), RenderError> {
            let Some(clip) = clip else {
                let calls = draw(self)?;
                self.stats.draw_calls = self.stats.draw_calls.saturating_add(calls);
                return Ok(());
            };
            if clip.corner_radius <= 0.0 {
                let bounds = d2d_rect(clip.bounds);
                // SAFETY: The clip rectangle remains valid until the matching Pop call below.
                unsafe {
                    self.context
                        .PushAxisAlignedClip(&raw const bounds, D2D1_ANTIALIAS_MODE_ALIASED);
                };
                let result = draw(self);
                // SAFETY: Balances the immediately preceding axis-aligned clip push.
                unsafe { self.context.PopAxisAlignedClip() };
                let calls = result?;
                self.stats.draw_calls = self.stats.draw_calls.saturating_add(calls);
                return Ok(());
            }
            let rounded = D2D1_ROUNDED_RECT {
                rect: d2d_rect(clip.bounds),
                radiusX: clip.corner_radius,
                radiusY: clip.corner_radius,
            };
            // SAFETY: The rounded rectangle pointer is valid for synchronous geometry creation.
            let rounded_geometry = unsafe {
                self.factory
                    .CreateRoundedRectangleGeometry(&raw const rounded)
            }
            .map_err(graphics_error)?;
            let geometry: ID2D1Geometry = rounded_geometry.cast().map_err(graphics_error)?;
            let mut parameters = D2D1_LAYER_PARAMETERS1 {
                contentBounds: d2d_rect(clip.bounds),
                geometricMask: ManuallyDrop::new(Some(geometry)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: identity_matrix(),
                opacity: 1.0,
                opacityBrush: ManuallyDrop::new(None::<ID2D1Brush>),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            // SAFETY: Layer parameters remain alive until the matching PopLayer below. A null
            // explicit layer asks Direct2D to manage a bounded temporary layer for this draw.
            unsafe {
                self.context.PushLayer(
                    &raw const parameters,
                    None::<&windows::Win32::Graphics::Direct2D::ID2D1Layer>,
                );
            }
            let result = draw(self);
            // SAFETY: Balances PushLayer before releasing the COM geometry stored in ManuallyDrop.
            unsafe { self.context.PopLayer() };
            // SAFETY: D2D1_LAYER_PARAMETERS1 is an ABI struct without Drop; this releases exactly
            // the one geometry reference moved into its ManuallyDrop field after D2D is done.
            unsafe { ManuallyDrop::drop(&mut parameters.geometricMask) };
            let calls = result?;
            self.stats.draw_calls = self.stats.draw_calls.saturating_add(calls);
            Ok(())
        }

        fn set_brush(&self, color: Color) {
            let color = d2d_color(color);
            // SAFETY: The brush and stack color value are live for this synchronous setter.
            unsafe { self.brush.SetColor(&raw const color) };
        }

        fn upload_atlases(&mut self, uploads: &[AtlasUpload]) -> Result<(), RenderError> {
            for upload in uploads {
                let expected = usize::try_from(upload.size.width)
                    .ok()
                    .and_then(|width| {
                        usize::try_from(upload.size.height)
                            .ok()
                            .and_then(|height| width.checked_mul(height))
                    })
                    .unwrap_or(usize::MAX);
                if upload.pixels.len() != expected {
                    return Err(RenderError::InvalidAtlasUpload {
                        atlas: upload.atlas.0,
                        generation: upload.generation,
                        expected,
                        actual: upload.pixels.len(),
                    });
                }
                if self
                    .atlases
                    .get(&upload.atlas)
                    .is_some_and(|cached| cached.generation == upload.generation)
                {
                    continue;
                }
                if !self.atlases.contains_key(&upload.atlas)
                    && self.atlases.len() == self.atlas_capacity.get()
                    && let Some(evicted) = self
                        .atlases
                        .iter()
                        .min_by_key(|(_, cached)| cached.last_used)
                        .map(|(atlas, _)| *atlas)
                {
                    self.atlases.remove(&evicted);
                    self.stats.atlas_evictions = self.stats.atlas_evictions.saturating_add(1);
                }
                let properties = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_A8_UNORM,
                        // A8 is an alpha-only opacity mask; IGNORE is not a supported Direct2D
                        // pairing for this format.
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                    colorContext: ManuallyDrop::new(None),
                };
                let pitch = upload.size.width;
                // SAFETY: Upload bytes contain exactly one tightly packed A8 page and remain live
                // for the synchronous bitmap creation call; Direct2D copies the data.
                let bitmap = unsafe {
                    self.context.CreateBitmap(
                        D2D_SIZE_U {
                            width: upload.size.width,
                            height: upload.size.height,
                        },
                        Some(upload.pixels.as_ptr().cast()),
                        pitch,
                        &raw const properties,
                    )
                }
                .map_err(graphics_error)?;
                self.atlas_clock = self.atlas_clock.saturating_add(1);
                self.atlases.insert(
                    upload.atlas,
                    CachedAtlas {
                        generation: upload.generation,
                        bitmap,
                        size: (upload.size.width, upload.size.height),
                        bytes: expected,
                        last_used: self.atlas_clock,
                    },
                );
                self.stats.atlas_uploads = self.stats.atlas_uploads.saturating_add(1);
            }
            self.refresh_cache_stats();
            Ok(())
        }

        fn validate_scene_atlases(&self, scene: &Scene) -> Result<(), RenderError> {
            for command in scene.commands() {
                if let DrawCommand::Glyphs { glyphs, .. } = command {
                    for glyph in glyphs.iter() {
                        if !self.atlases.contains_key(&glyph.atlas) {
                            return Err(RenderError::MissingAtlas {
                                atlas: glyph.atlas.0,
                            });
                        }
                    }
                }
            }
            Ok(())
        }

        fn refresh_cache_stats(&mut self) {
            self.stats.cached_atlases = self.atlases.len();
            self.stats.cached_atlas_bytes = self.atlases.values().map(|atlas| atlas.bytes).sum();
        }
    }

    fn create_device(
        desc: &DXGI_SWAP_CHAIN_DESC,
        levels: &[windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL],
    ) -> Result<(ID3D11Device, IDXGISwapChain), RenderError> {
        create_device_for_driver(desc, levels, D3D_DRIVER_TYPE_HARDWARE)
            .or_else(|_| create_device_for_driver(desc, levels, D3D_DRIVER_TYPE_WARP))
            .map_err(graphics_error)
    }

    fn create_device_for_driver(
        desc: &DXGI_SWAP_CHAIN_DESC,
        levels: &[windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL],
        driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
    ) -> WindowsResult<(ID3D11Device, IDXGISwapChain)> {
        let mut device = None;
        let mut swap_chain = None;
        // SAFETY: All out pointers reference initialized Options, `desc` contains a live HWND and
        // non-zero dimensions, and returned COM objects are reference counted by `windows`.
        unsafe {
            D3D11CreateDeviceAndSwapChain(
                None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(levels),
                D3D11_SDK_VERSION,
                Some(desc),
                Some(&raw mut swap_chain),
                Some(&raw mut device),
                None,
                None,
            )
        }?;
        let device = device.ok_or_else(windows::core::Error::empty)?;
        let swap_chain = swap_chain.ok_or_else(windows::core::Error::empty)?;
        Ok((device, swap_chain))
    }

    fn create_target(
        context: &ID2D1DeviceContext,
        swap_chain: &IDXGISwapChain,
        scale: f32,
    ) -> Result<ID2D1Bitmap1, RenderError> {
        // SAFETY: Buffer zero exists for a live swap chain and is queried as its DXGI surface.
        let surface: IDXGISurface = unsafe { swap_chain.GetBuffer(0) }.map_err(graphics_error)?;
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0 * scale,
            dpiY: 96.0 * scale,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        // SAFETY: The DXGI surface and properties remain valid through synchronous bitmap
        // creation; the returned target retains the surface.
        unsafe { context.CreateBitmapFromDxgiSurface(&surface, Some(&raw const properties)) }
            .map_err(graphics_error)
    }

    fn d2d_rect(rect: Rect) -> D2D_RECT_F {
        D2D_RECT_F {
            left: rect.min_x(),
            top: rect.min_y(),
            right: rect.max_x(),
            bottom: rect.max_y(),
        }
    }

    const fn d2d_color(color: Color) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }

    const fn identity_matrix() -> Matrix3x2 {
        Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        }
    }
}

#[cfg(target_os = "windows")]
pub use platform::D3d11Renderer;

#[cfg(not(target_os = "windows"))]
pub struct D3d11Renderer;

#[cfg(not(target_os = "windows"))]
impl D3d11Renderer {
    /// Reports that the D3D11 renderer is unavailable on this target.
    ///
    /// # Errors
    ///
    /// Always returns [`RenderError::UnsupportedPlatform`].
    pub fn new() -> Result<Self, RenderError> {
        Err(RenderError::UnsupportedPlatform)
    }
}
