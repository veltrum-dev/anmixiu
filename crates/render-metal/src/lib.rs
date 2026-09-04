//! Metal renderer for immutable [`anmixiu_scene::Scene`] snapshots.
//!
//! A render call encodes all commands into one command buffer and performs at most
//! one submission. R8 glyph textures are cached by `(AtlasId, generation)` in a
//! hard-capacity LRU; a new generation replaces the previous texture for that id.

#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceSize {
    width: u32,
    height: u32,
}

impl SurfaceSize {
    /// Creates a non-empty render target size.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidSurfaceSize`] for a zero dimension.
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

    /// Checks whether a drawable has the exact configured physical size.
    ///
    /// # Errors
    ///
    /// Returns a structured stale-surface error instead of allowing mixed-scale rendering.
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
    pub submitted_frames: u64,
    pub drawable_misses: u64,
    pub stale_surface_misses: u64,
    pub draw_calls: u64,
    pub atlas_uploads: u64,
    pub cached_atlases: usize,
    pub cached_atlas_bytes: usize,
    /// Frames that used the intermediate compositor rather than the direct fast path.
    pub composited_frames: u64,
    /// Ordered backdrop blur operations encoded across all submitted frames.
    pub backdrop_blur_operations: u64,
    /// Element-subtree filter blur operations encoded across all submitted frames.
    pub filter_blur_operations: u64,
    /// Bytes retained by the bounded in-flight compositor texture ring.
    pub compositor_texture_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameOutcome {
    Presented,
    DrawableUnavailable {
        retry_immediately: bool,
    },
    CompositorBusy {
        retry_immediately: bool,
    },
    SurfaceOutOfDate {
        expected: SurfaceSize,
        actual: SurfaceSize,
        retry_immediately: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OffscreenImage {
    size: SurfaceSize,
    rgba: Vec<u8>,
}

impl OffscreenImage {
    #[must_use]
    pub const fn size(&self) -> SurfaceSize {
        self.size
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.rgba
    }

    #[must_use]
    /// Returns one pixel.
    ///
    /// # Panics
    ///
    /// Panics when `(x, y)` is outside [`Self::size`].
    pub fn pixel_rgba(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.size.width && y < self.size.height);
        let start = ((y * self.size.width + x) * 4) as usize;
        self.rgba[start..start + 4]
            .try_into()
            .expect("four-byte RGBA pixel")
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum RenderError {
    #[error("surface dimensions must be non-zero, got {width}x{height}")]
    InvalidSurfaceSize { width: u32, height: u32 },
    #[error("drawable surface dimensions exceed u32: {width}x{height}")]
    SurfaceDimensionOverflow { width: u64, height: u64 },
    #[error("drawable surface is out of date: expected {expected:?}, got {actual:?}")]
    SurfaceOutOfDate {
        expected: SurfaceSize,
        actual: SurfaceSize,
    },
    #[error("drawable dimensions exceed the supported u32 range: {width}x{height}")]
    SurfaceTooLarge { width: u64, height: u64 },
    #[error("atlas texture capacity must be non-zero")]
    InvalidAtlasCapacity,
    #[error("Metal shader compilation failed: {0}")]
    ShaderCompilation(String),
    #[error("Metal pipeline creation failed: {0}")]
    PipelineCreation(String),
    #[error("atlas {atlas} generation {generation} has {actual} bytes, expected {expected}")]
    InvalidAtlasUpload {
        atlas: u64,
        generation: u64,
        expected: usize,
        actual: usize,
    },
    #[error("scene references atlas {atlas}, but it has not been uploaded")]
    MissingAtlas { atlas: u64 },
    #[error("a scene may contain at most 64 backdrop blur operations")]
    TooManyBackdropBlurs,
    #[error("a scene may contain at most 64 filter blur operations")]
    TooManyFilterBlurs,
    #[error("filter blur nesting may not exceed 8 layers")]
    FilterBlurNestingTooDeep,
    #[error("compositor textures exceed the 256 MiB hard budget")]
    CompositorBudgetExceeded,
    #[error("all three bounded compositor slots are still in flight")]
    CompositorBusy,
    #[error("compositor resources were not prepared for the scene")]
    CompositorResourcesUnavailable,
    #[error("Metal command buffer failed")]
    CommandBufferFailed,
    #[error("Metal is only available on macOS")]
    UnsupportedPlatform,
}

#[cfg(target_os = "macos")]
#[allow(clippy::cast_precision_loss)]
mod platform {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::mem::{size_of, size_of_val};

    use anmixiu_scene::{
        AtlasId, AtlasUpload, Clip, Color, DrawCommand, Glyph, MAX_BACKDROP_BLUR_SIGMA,
        MAX_FILTER_BLUR_SIGMA, Rect, Scene,
    };
    use core_graphics::geometry::CGSize;
    use metal::{
        Buffer, CommandQueue, CompileOptions, Device, MTLClearColor, MTLCommandBufferStatus,
        MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLResourceOptions,
        MTLSize, MTLStorageMode, MTLStoreAction, MTLTextureUsage, MTLViewport, MetalDrawableRef,
        MetalLayerRef, RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState,
        Texture, TextureDescriptor, TextureRef,
    };

    use super::{
        FrameOutcome, OffscreenImage, RenderError, RenderStats, RendererConfig, SurfaceSize,
    };

    const SHADER_SOURCE: &str = include_str!("gui.metal");

    #[rustfmt::skip]
    const UNIT_QUAD: [[f32; 2]; 6] = [
        [0.0, 0.0], [1.0, 0.0], [0.0, 1.0],
        [0.0, 1.0], [1.0, 0.0], [1.0, 1.0],
    ];

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DrawUniforms {
        color: [f32; 4],
        bounds: [f32; 4],
        clip_rect: [f32; 4],
        misc: [f32; 4],
        uv_rect: [f32; 4],
        sample_uv_bounds: [f32; 4],
    }

    struct CachedAtlas {
        generation: u64,
        texture: Texture,
        last_used: u64,
    }

    const COMPOSITOR_SLOT_COUNT: usize = 3;
    const MAX_BACKDROP_BLURS_PER_FRAME: usize = 64;
    const MAX_FILTER_BLURS_PER_FRAME: usize = 64;
    const MAX_FILTER_BLUR_DEPTH: usize = 8;
    const TARGET_DOWNSAMPLED_SIGMA: f32 = 8.0;
    const MAX_DOWNSAMPLE: u32 = 16;
    const COMPOSITOR_TEXTURE_BUDGET: usize = 256 * 1024 * 1024;

    struct BlurTextures {
        size: SurfaceSize,
        format: MTLPixelFormat,
        first: Texture,
        second: Texture,
    }

    struct CompositeSlot {
        in_flight: Option<metal::CommandBuffer>,
        scene_size: Option<SurfaceSize>,
        scene_format: Option<MTLPixelFormat>,
        scene: Option<Texture>,
        blur: Option<BlurTextures>,
        filter_layers: Vec<Texture>,
    }

    impl CompositeSlot {
        fn new() -> Self {
            Self {
                in_flight: None,
                scene_size: None,
                scene_format: None,
                scene: None,
                blur: None,
                filter_layers: Vec::new(),
            }
        }
    }

    #[derive(Clone, Copy)]
    struct BlurPlan {
        sample_bounds: Rect,
        scratch_size: SurfaceSize,
        downsample: u32,
        effective_sigma: f32,
    }

    struct RendererPipelines {
        rgba: RenderPipelineState,
        bgra: RenderPipelineState,
        rgba_border: RenderPipelineState,
        bgra_border: RenderPipelineState,
        rgba_image: RenderPipelineState,
        bgra_image: RenderPipelineState,
        rgba_downsample: RenderPipelineState,
        bgra_downsample: RenderPipelineState,
        rgba_blur: RenderPipelineState,
        bgra_blur: RenderPipelineState,
        rgba_filter_blur: RenderPipelineState,
        bgra_filter_blur: RenderPipelineState,
        rgba_filter_composite: RenderPipelineState,
        bgra_filter_composite: RenderPipelineState,
    }

    enum EncodeOutcome {
        Encoded(metal::CommandBuffer),
        CompositorBusy,
    }

    pub struct MetalRenderer {
        device: Device,
        queue: CommandQueue,
        rgba_pipeline: RenderPipelineState,
        bgra_pipeline: RenderPipelineState,
        rgba_border_pipeline: RenderPipelineState,
        bgra_border_pipeline: RenderPipelineState,
        rgba_image_pipeline: RenderPipelineState,
        bgra_image_pipeline: RenderPipelineState,
        rgba_downsample_pipeline: RenderPipelineState,
        bgra_downsample_pipeline: RenderPipelineState,
        rgba_blur_pipeline: RenderPipelineState,
        bgra_blur_pipeline: RenderPipelineState,
        rgba_filter_blur_pipeline: RenderPipelineState,
        bgra_filter_blur_pipeline: RenderPipelineState,
        rgba_filter_composite_pipeline: RenderPipelineState,
        bgra_filter_composite_pipeline: RenderPipelineState,
        unit_quad: Buffer,
        atlas_capacity: usize,
        atlas_textures: HashMap<AtlasId, CachedAtlas>,
        use_clock: u64,
        stats: RenderStats,
        configured_surface: Cell<Option<SurfaceSize>>,
        compositor_slots: Vec<CompositeSlot>,
    }

    impl MetalRenderer {
        /// Creates a renderer, returning `Ok(None)` when this Mac has no Metal device.
        ///
        /// # Errors
        ///
        /// Returns a shader or pipeline error when Metal initialization fails.
        pub fn new() -> Result<Option<Self>, RenderError> {
            Self::with_config(RendererConfig::default())
        }

        /// Creates a renderer with hard cache limits.
        ///
        /// # Errors
        ///
        /// Returns an error for zero cache capacity or failed Metal shader/pipeline creation.
        pub fn with_config(config: RendererConfig) -> Result<Option<Self>, RenderError> {
            if config.atlas_texture_capacity == 0 {
                return Err(RenderError::InvalidAtlasCapacity);
            }
            let Some(device) = Device::system_default() else {
                return Ok(None);
            };
            let options = CompileOptions::new();
            let library = device
                .new_library_with_source(SHADER_SOURCE, &options)
                .map_err(RenderError::ShaderCompilation)?;
            let vertex = library
                .get_function("gui_vertex", None)
                .map_err(RenderError::ShaderCompilation)?;
            let pipelines = renderer_pipelines(&device, &library, &vertex)?;
            let queue = device.new_command_queue();
            let unit_quad = device.new_buffer_with_data(
                UNIT_QUAD.as_ptr().cast(),
                size_of_val(&UNIT_QUAD) as u64,
                MTLResourceOptions::StorageModeShared,
            );
            Ok(Some(Self {
                device,
                queue,
                rgba_pipeline: pipelines.rgba,
                bgra_pipeline: pipelines.bgra,
                rgba_border_pipeline: pipelines.rgba_border,
                bgra_border_pipeline: pipelines.bgra_border,
                rgba_image_pipeline: pipelines.rgba_image,
                bgra_image_pipeline: pipelines.bgra_image,
                rgba_downsample_pipeline: pipelines.rgba_downsample,
                bgra_downsample_pipeline: pipelines.bgra_downsample,
                rgba_blur_pipeline: pipelines.rgba_blur,
                bgra_blur_pipeline: pipelines.bgra_blur,
                rgba_filter_blur_pipeline: pipelines.rgba_filter_blur,
                bgra_filter_blur_pipeline: pipelines.bgra_filter_blur,
                rgba_filter_composite_pipeline: pipelines.rgba_filter_composite,
                bgra_filter_composite_pipeline: pipelines.bgra_filter_composite,
                unit_quad,
                atlas_capacity: config.atlas_texture_capacity,
                atlas_textures: HashMap::with_capacity(config.atlas_texture_capacity),
                use_clock: 0,
                stats: RenderStats::default(),
                configured_surface: Cell::new(None),
                compositor_slots: (0..COMPOSITOR_SLOT_COUNT)
                    .map(|_| CompositeSlot::new())
                    .collect(),
            }))
        }

        #[must_use]
        pub fn stats(&self) -> RenderStats {
            RenderStats {
                cached_atlases: self.atlas_textures.len(),
                cached_atlas_bytes: self
                    .atlas_textures
                    .values()
                    .map(|cached| {
                        usize::try_from(cached.texture.width())
                            .unwrap_or(usize::MAX)
                            .saturating_mul(
                                usize::try_from(cached.texture.height()).unwrap_or(usize::MAX),
                            )
                    })
                    .sum(),
                compositor_texture_bytes: self
                    .compositor_slots
                    .iter()
                    .map(composite_slot_bytes)
                    .sum(),
                ..self.stats
            }
        }

        /// Configures a `CAMetalLayer` for a physical drawable size and Retina scale.
        pub fn configure_layer(&self, layer: &MetalLayerRef, size: SurfaceSize, scale: f32) {
            self.configured_surface.set(Some(size));
            layer.set_device(&self.device);
            layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm_sRGB);
            layer.set_framebuffer_only(true);
            // Coordinate drawable presentation with AppKit's live-resize transaction. Returning
            // before the command buffer is scheduled lets Core Animation stretch the previous
            // drawable while the new one is still queued, which is visible as resize jitter.
            layer.set_presents_with_transaction(true);
            layer.set_display_sync_enabled(true);
            layer.set_contents_scale(f64::from(scale.max(1.0)));
            layer.set_drawable_size(CGSize::new(f64::from(size.width), f64::from(size.height)));
        }

        /// Acquires and renders one layer drawable. A missing drawable is diagnostic only;
        /// callers wait for the next display/window wakeup instead of spinning.
        ///
        /// # Errors
        ///
        /// Returns an atlas validation or Metal encoding error.
        pub fn render_layer(
            &mut self,
            layer: &MetalLayerRef,
            scene: &Scene,
            scale: f32,
        ) -> Result<FrameOutcome, RenderError> {
            let Some(drawable) = layer.next_drawable() else {
                self.stats.drawable_misses += 1;
                return Ok(FrameOutcome::DrawableUnavailable {
                    retry_immediately: false,
                });
            };
            let texture = drawable.texture();
            let texture_width = texture.width();
            let texture_height = texture.height();
            let actual = SurfaceSize::new(
                u32::try_from(texture_width).map_err(|_| {
                    RenderError::SurfaceDimensionOverflow {
                        width: texture_width,
                        height: texture_height,
                    }
                })?,
                u32::try_from(texture_height).map_err(|_| {
                    RenderError::SurfaceDimensionOverflow {
                        width: texture_width,
                        height: texture_height,
                    }
                })?,
            )?;
            if let Some(expected) = self.configured_surface.get()
                && expected != actual
            {
                self.stats.stale_surface_misses += 1;
                return Ok(FrameOutcome::SurfaceOutOfDate {
                    expected,
                    actual,
                    retry_immediately: false,
                });
            }
            let presents_with_transaction = layer.presents_with_transaction();
            self.render_drawable_inner(Some(drawable), scene, scale, presents_with_transaction)
        }

        /// Renders a caller-provided drawable at scale 1, or records a non-spinning miss.
        ///
        /// # Errors
        ///
        /// Returns an atlas validation or Metal encoding error.
        pub fn render_optional_drawable(
            &mut self,
            drawable: Option<&MetalDrawableRef>,
            scene: &Scene,
        ) -> Result<FrameOutcome, RenderError> {
            self.render_optional_drawable_scaled(drawable, scene, 1.0)
        }

        /// Renders a caller-provided drawable using logical-to-physical `scale`.
        ///
        /// # Errors
        ///
        /// Returns an atlas validation, surface-size, or Metal encoding error.
        pub fn render_optional_drawable_scaled(
            &mut self,
            drawable: Option<&MetalDrawableRef>,
            scene: &Scene,
            scale: f32,
        ) -> Result<FrameOutcome, RenderError> {
            self.render_drawable_inner(drawable, scene, scale, false)
        }

        /// Encodes and presents `drawable`. When `presents_with_transaction` is set the drawable
        /// is presented on the CPU after the command buffer is scheduled, so the present lands in
        /// the same Core Animation transaction as any pending layer-geometry change. Otherwise the
        /// GPU registers the present asynchronously via the command buffer.
        fn render_drawable_inner(
            &mut self,
            drawable: Option<&MetalDrawableRef>,
            scene: &Scene,
            scale: f32,
            presents_with_transaction: bool,
        ) -> Result<FrameOutcome, RenderError> {
            let Some(drawable) = drawable else {
                self.stats.drawable_misses += 1;
                return Ok(FrameOutcome::DrawableUnavailable {
                    retry_immediately: false,
                });
            };
            self.upload_atlases(scene.atlas_uploads())?;
            let texture = drawable.texture();
            let texture_width = texture.width();
            let texture_height = texture.height();
            let width = u32::try_from(texture_width).map_err(|_| RenderError::SurfaceTooLarge {
                width: texture_width,
                height: texture_height,
            })?;
            let height =
                u32::try_from(texture_height).map_err(|_| RenderError::SurfaceTooLarge {
                    width: texture_width,
                    height: texture_height,
                })?;
            let size = SurfaceSize::new(width, height)?;
            let command_buffer =
                match self.encode(scene, texture, size, scale, MTLPixelFormat::BGRA8Unorm_sRGB)? {
                    EncodeOutcome::Encoded(command_buffer) => command_buffer,
                    EncodeOutcome::CompositorBusy => {
                        return Ok(FrameOutcome::CompositorBusy {
                            retry_immediately: false,
                        });
                    }
                };
            if presents_with_transaction {
                // Transaction mode: commit and wait until the GPU has scheduled the work, then
                // present synchronously so Core Animation swaps the drawable in the same
                // transaction that applied the new layer bounds. Presenting through the command
                // buffer here would defer the swap to a later frame, letting CA stretch the stale
                // drawable over the resized layer — the visible resize jitter.
                command_buffer.commit();
                command_buffer.wait_until_scheduled();
                drawable.present();
            } else {
                command_buffer.present_drawable(drawable);
                command_buffer.commit();
                // Synchronize only until the GPU has accepted the frame. Waiting for completion
                // here would block the AppKit thread and is unnecessary for presentation
                // correctness.
                command_buffer.wait_until_scheduled();
            }
            self.stats.submitted_frames += 1;
            Ok(FrameOutcome::Presented)
        }

        /// Renders into a CPU-readable RGBA8 texture and waits only for explicit test/readback.
        ///
        /// # Errors
        ///
        /// Returns an atlas validation, Metal encoding, or command-buffer error.
        pub fn render_offscreen(
            &mut self,
            scene: &Scene,
            size: SurfaceSize,
        ) -> Result<OffscreenImage, RenderError> {
            self.render_offscreen_scaled(scene, size, 1.0)
        }

        /// Renders and reads back at a logical-to-physical scale.
        ///
        /// # Errors
        ///
        /// Returns an atlas validation, Metal encoding, or command-buffer error.
        pub fn render_offscreen_scaled(
            &mut self,
            scene: &Scene,
            size: SurfaceSize,
            scale: f32,
        ) -> Result<OffscreenImage, RenderError> {
            self.upload_atlases(scene.atlas_uploads())?;
            let descriptor = TextureDescriptor::new();
            descriptor.set_width(u64::from(size.width));
            descriptor.set_height(u64::from(size.height));
            descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm_sRGB);
            descriptor.set_storage_mode(MTLStorageMode::Shared);
            descriptor.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            let texture = self.device.new_texture(&descriptor);
            let command_buffer = match self.encode(
                scene,
                &texture,
                size,
                scale,
                MTLPixelFormat::RGBA8Unorm_sRGB,
            )? {
                EncodeOutcome::Encoded(command_buffer) => command_buffer,
                EncodeOutcome::CompositorBusy => return Err(RenderError::CompositorBusy),
            };
            command_buffer.commit();
            command_buffer.wait_until_completed();
            if command_buffer.status() == MTLCommandBufferStatus::Error {
                return Err(RenderError::CommandBufferFailed);
            }
            self.stats.submitted_frames += 1;
            let mut rgba = vec![0; size.width as usize * size.height as usize * 4];
            texture.get_bytes(
                rgba.as_mut_ptr().cast(),
                u64::from(size.width) * 4,
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize {
                        width: u64::from(size.width),
                        height: u64::from(size.height),
                        depth: 1,
                    },
                },
                0,
            );
            Ok(OffscreenImage { size, rgba })
        }

        fn upload_atlases(&mut self, uploads: &[AtlasUpload]) -> Result<(), RenderError> {
            for upload in uploads {
                let expected = upload.size.width as usize * upload.size.height as usize;
                if upload.pixels.len() != expected {
                    return Err(RenderError::InvalidAtlasUpload {
                        atlas: upload.atlas.0,
                        generation: upload.generation,
                        expected,
                        actual: upload.pixels.len(),
                    });
                }
                self.use_clock = self.use_clock.wrapping_add(1);
                if let Some(cached) = self.atlas_textures.get_mut(&upload.atlas) {
                    cached.last_used = self.use_clock;
                    if cached.generation == upload.generation {
                        continue;
                    }
                } else if self.atlas_textures.len() == self.atlas_capacity {
                    let oldest = self
                        .atlas_textures
                        .iter()
                        .min_by_key(|(_, cached)| cached.last_used)
                        .map(|(id, _)| *id)
                        .expect("non-zero full atlas cache has an entry");
                    self.atlas_textures.remove(&oldest);
                }
                let descriptor = TextureDescriptor::new();
                descriptor.set_width(u64::from(upload.size.width));
                descriptor.set_height(u64::from(upload.size.height));
                descriptor.set_pixel_format(MTLPixelFormat::R8Unorm);
                descriptor.set_storage_mode(MTLStorageMode::Shared);
                descriptor.set_usage(MTLTextureUsage::ShaderRead);
                let texture = self.device.new_texture(&descriptor);
                texture.replace_region(
                    MTLRegion {
                        origin: MTLOrigin { x: 0, y: 0, z: 0 },
                        size: MTLSize {
                            width: u64::from(upload.size.width),
                            height: u64::from(upload.size.height),
                            depth: 1,
                        },
                    },
                    0,
                    upload.pixels.as_ptr().cast(),
                    u64::from(upload.size.width),
                );
                self.atlas_textures.insert(
                    upload.atlas,
                    CachedAtlas {
                        generation: upload.generation,
                        texture,
                        last_used: self.use_clock,
                    },
                );
                self.stats.atlas_uploads += 1;
            }
            Ok(())
        }

        fn encode(
            &mut self,
            scene: &Scene,
            target: &TextureRef,
            size: SurfaceSize,
            scale: f32,
            format: MTLPixelFormat,
        ) -> Result<EncodeOutcome, RenderError> {
            let scale = valid_scale(scale);
            if !scene.requires_compositing() {
                return self
                    .encode_direct(scene, target, size, scale, format)
                    .map(EncodeOutcome::Encoded);
            }

            let Some(slot_index) = self.acquire_compositor_slot() else {
                return Ok(EncodeOutcome::CompositorBusy);
            };
            self.prepare_compositor_slot(slot_index, scene, size, scale, format)?;
            let command_buffer =
                self.encode_composited(scene, target, size, scale, format, slot_index)?;
            let slot = self
                .compositor_slots
                .get_mut(slot_index)
                .ok_or(RenderError::CompositorResourcesUnavailable)?;
            slot.in_flight = Some(command_buffer.clone());
            Ok(EncodeOutcome::Encoded(command_buffer))
        }

        fn encode_direct(
            &mut self,
            scene: &Scene,
            target: &TextureRef,
            size: SurfaceSize,
            scale: f32,
            format: MTLPixelFormat,
        ) -> Result<metal::CommandBuffer, RenderError> {
            let command_buffer = self.queue.new_command_buffer().to_owned();
            let encoder = begin_render_pass(
                &command_buffer,
                target,
                MTLLoadAction::Clear,
                MTLClearColor::new(0.0, 0.0, 0.0, 0.0),
            )?;
            self.configure_primitive_encoder(encoder, size, format);
            let mut border_pipeline_selected = false;
            for command in scene.commands() {
                self.encode_primitive(
                    encoder,
                    command,
                    scale,
                    format,
                    &mut border_pipeline_selected,
                )?;
            }
            encoder.end_encoding();
            Ok(command_buffer)
        }

        fn acquire_compositor_slot(&mut self) -> Option<usize> {
            for slot in &mut self.compositor_slots {
                let finished = slot.in_flight.as_ref().is_some_and(|command_buffer| {
                    matches!(
                        command_buffer.status(),
                        MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
                    )
                });
                if finished {
                    slot.in_flight = None;
                }
            }
            self.compositor_slots
                .iter()
                .position(|slot| slot.in_flight.is_none())
        }

        fn prepare_compositor_slot(
            &mut self,
            slot_index: usize,
            scene: &Scene,
            size: SurfaceSize,
            scale: f32,
            format: MTLPixelFormat,
        ) -> Result<(), RenderError> {
            for (index, slot) in self.compositor_slots.iter_mut().enumerate() {
                if index != slot_index
                    && slot.in_flight.is_none()
                    && (slot.scene_size != Some(size) || slot.scene_format != Some(format))
                {
                    slot.scene = None;
                    slot.scene_size = None;
                    slot.scene_format = None;
                    slot.blur = None;
                    slot.filter_layers.clear();
                }
            }
            let requirements = compositor_requirements(scene.commands(), size, scale)?;
            if requirements.backdrop_blur_count > MAX_BACKDROP_BLURS_PER_FRAME {
                return Err(RenderError::TooManyBackdropBlurs);
            }
            if requirements.filter_blur_count > MAX_FILTER_BLURS_PER_FRAME {
                return Err(RenderError::TooManyFilterBlurs);
            }
            let planned_scratch_size = requirements.scratch_size;
            let desired_slot_bytes =
                compositor_plan_bytes(size, planned_scratch_size, requirements.filter_layer_depth);
            let retained_other_bytes = self
                .compositor_slots
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != slot_index)
                .map(|(_, slot)| composite_slot_bytes(slot))
                .sum::<usize>();
            if retained_other_bytes.saturating_add(desired_slot_bytes) > COMPOSITOR_TEXTURE_BUDGET {
                return Err(RenderError::CompositorBudgetExceeded);
            }

            {
                let slot = self
                    .compositor_slots
                    .get_mut(slot_index)
                    .ok_or(RenderError::CompositorResourcesUnavailable)?;
                if slot.scene_size != Some(size) || slot.scene_format != Some(format) {
                    slot.scene = Some(color_texture(&self.device, size, format));
                    slot.scene_size = Some(size);
                    slot.scene_format = Some(format);
                    slot.blur = None;
                    slot.filter_layers.clear();
                }
                if let Some(scratch_size) = planned_scratch_size {
                    if slot.blur.as_ref().is_none_or(|textures| {
                        textures.size != scratch_size || textures.format != format
                    }) {
                        slot.blur = Some(blur_textures(&self.device, scratch_size, format));
                    }
                } else {
                    slot.blur = None;
                }
                if slot.filter_layers.len() != requirements.filter_layer_depth {
                    slot.filter_layers = (0..requirements.filter_layer_depth)
                        .map(|_| color_texture(&self.device, size, format))
                        .collect();
                }
            }
            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        fn encode_composited(
            &mut self,
            scene: &Scene,
            target: &TextureRef,
            size: SurfaceSize,
            scale: f32,
            format: MTLPixelFormat,
            slot_index: usize,
        ) -> Result<metal::CommandBuffer, RenderError> {
            let scene_texture = self
                .compositor_slots
                .get(slot_index)
                .and_then(|slot| slot.scene.as_ref())
                .map(|texture| TextureRef::to_owned(texture))
                .ok_or(RenderError::CompositorResourcesUnavailable)?;
            let command_buffer = self.queue.new_command_buffer().to_owned();
            self.encode_commands(
                &command_buffer,
                &scene_texture,
                scene.commands(),
                size,
                scale,
                format,
                slot_index,
                0,
                MTLLoadAction::Clear,
            )?;

            let final_uniforms = image_uniforms(
                Rect::new(
                    anmixiu_scene::Point::new(0.0, 0.0),
                    anmixiu_scene::Size::new(size.width as f32, size.height as f32),
                ),
                [0.0, 0.0, 1.0, 1.0],
                0.0,
                None,
                1.0,
            );
            encode_texture_quad(
                &command_buffer,
                target,
                size,
                MTLLoadAction::Clear,
                image_pipeline(self, format),
                &self.unit_quad,
                &scene_texture,
                &final_uniforms,
            )?;
            self.stats.draw_calls = self.stats.draw_calls.saturating_add(1);
            self.stats.composited_frames = self.stats.composited_frames.saturating_add(1);
            Ok(command_buffer)
        }

        #[allow(clippy::too_many_arguments)]
        fn encode_commands(
            &mut self,
            command_buffer: &metal::CommandBufferRef,
            target: &TextureRef,
            commands: &[DrawCommand],
            size: SurfaceSize,
            scale: f32,
            format: MTLPixelFormat,
            slot_index: usize,
            filter_depth: usize,
            load_action: MTLLoadAction,
        ) -> Result<(), RenderError> {
            let mut encoder = begin_render_pass(
                command_buffer,
                target,
                load_action,
                MTLClearColor::new(0.0, 0.0, 0.0, 0.0),
            )?;
            self.configure_primitive_encoder(encoder, size, format);
            let mut border_pipeline_selected = false;
            for command in commands {
                match command {
                    DrawCommand::BackdropBlur {
                        bounds,
                        corner_radius,
                        clip,
                        ..
                    } => {
                        let Some(plan) = blur_plan(command, size, scale) else {
                            continue;
                        };
                        encoder.end_encoding();
                        let (first, second) = self.blur_textures_for_slot(slot_index)?;
                        self.encode_backdrop_blur(
                            command_buffer,
                            target,
                            &first,
                            &second,
                            plan,
                            *bounds,
                            *corner_radius,
                            *clip,
                            size,
                            scale,
                            format,
                        )?;
                        encoder = begin_render_pass(
                            command_buffer,
                            target,
                            MTLLoadAction::Load,
                            MTLClearColor::new(0.0, 0.0, 0.0, 0.0),
                        )?;
                        self.configure_primitive_encoder(encoder, size, format);
                        border_pipeline_selected = false;
                    }
                    DrawCommand::FilterBlur {
                        sigma,
                        clip,
                        commands,
                    } => {
                        encoder.end_encoding();
                        if let Some(plan) = filter_blur_plan(*sigma, size, scale) {
                            self.encode_filter_blur(
                                command_buffer,
                                target,
                                commands,
                                *clip,
                                plan,
                                size,
                                scale,
                                format,
                                slot_index,
                                filter_depth,
                            )?;
                        } else {
                            self.encode_commands(
                                command_buffer,
                                target,
                                commands,
                                size,
                                scale,
                                format,
                                slot_index,
                                filter_depth,
                                MTLLoadAction::Load,
                            )?;
                        }
                        encoder = begin_render_pass(
                            command_buffer,
                            target,
                            MTLLoadAction::Load,
                            MTLClearColor::new(0.0, 0.0, 0.0, 0.0),
                        )?;
                        self.configure_primitive_encoder(encoder, size, format);
                        border_pipeline_selected = false;
                    }
                    DrawCommand::SolidQuad { .. }
                    | DrawCommand::RoundedQuad { .. }
                    | DrawCommand::RoundedBorder { .. }
                    | DrawCommand::Glyphs { .. } => self.encode_primitive(
                        encoder,
                        command,
                        scale,
                        format,
                        &mut border_pipeline_selected,
                    )?,
                }
            }
            encoder.end_encoding();
            Ok(())
        }

        fn blur_textures_for_slot(
            &self,
            slot_index: usize,
        ) -> Result<(Texture, Texture), RenderError> {
            self.compositor_slots
                .get(slot_index)
                .and_then(|slot| slot.blur.as_ref())
                .map(|textures| {
                    (
                        TextureRef::to_owned(&textures.first),
                        TextureRef::to_owned(&textures.second),
                    )
                })
                .ok_or(RenderError::CompositorResourcesUnavailable)
        }

        #[allow(clippy::too_many_arguments)]
        fn encode_filter_blur(
            &mut self,
            command_buffer: &metal::CommandBufferRef,
            target: &TextureRef,
            commands: &[DrawCommand],
            clip: Option<Clip>,
            plan: BlurPlan,
            scene_size: SurfaceSize,
            scale: f32,
            format: MTLPixelFormat,
            slot_index: usize,
            filter_depth: usize,
        ) -> Result<(), RenderError> {
            let layer = self
                .compositor_slots
                .get(slot_index)
                .and_then(|slot| slot.filter_layers.get(filter_depth))
                .map(|texture| TextureRef::to_owned(texture))
                .ok_or(RenderError::CompositorResourcesUnavailable)?;
            self.encode_commands(
                command_buffer,
                &layer,
                commands,
                scene_size,
                scale,
                format,
                slot_index,
                filter_depth.saturating_add(1),
                MTLLoadAction::Clear,
            )?;
            let (first, second) = self.blur_textures_for_slot(slot_index)?;
            let allocated_scratch = self.encode_blur_passes(
                command_buffer,
                &layer,
                &first,
                &second,
                plan,
                scene_size,
                format,
                true,
            )?;
            let logical_surface = Rect::new(
                anmixiu_scene::Point::new(0.0, 0.0),
                anmixiu_scene::Size::new(
                    scene_size.width as f32 / scale,
                    scene_size.height as f32 / scale,
                ),
            );
            let content_uv_scale = [
                plan.scratch_size.width as f32 / allocated_scratch.width as f32,
                plan.scratch_size.height as f32 / allocated_scratch.height as f32,
            ];
            let mut composite =
                image_uniforms(logical_surface, [0.0, 0.0, 1.0, 1.0], 0.0, clip, scale);
            composite.uv_rect = [0.0, 0.0, content_uv_scale[0], content_uv_scale[1]];
            composite.sample_uv_bounds = content_texel_bounds(plan.scratch_size, allocated_scratch);
            encode_texture_quad(
                command_buffer,
                target,
                scene_size,
                MTLLoadAction::Load,
                filter_composite_pipeline(self, format),
                &self.unit_quad,
                &first,
                &composite,
            )?;
            self.stats.draw_calls = self.stats.draw_calls.saturating_add(4);
            self.stats.filter_blur_operations = self.stats.filter_blur_operations.saturating_add(1);
            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        fn encode_backdrop_blur(
            &mut self,
            command_buffer: &metal::CommandBufferRef,
            scene_texture: &TextureRef,
            first: &TextureRef,
            second: &TextureRef,
            plan: BlurPlan,
            bounds: Rect,
            corner_radius: f32,
            clip: Option<Clip>,
            scene_size: SurfaceSize,
            scale: f32,
            format: MTLPixelFormat,
        ) -> Result<(), RenderError> {
            let allocated_scratch = self.encode_blur_passes(
                command_buffer,
                scene_texture,
                first,
                second,
                plan,
                scene_size,
                format,
                false,
            )?;
            let physical_bounds = Rect::new(
                anmixiu_scene::Point::new(bounds.origin.x * scale, bounds.origin.y * scale),
                anmixiu_scene::Size::new(bounds.size.width * scale, bounds.size.height * scale),
            );
            let content_uv_scale = [
                plan.scratch_size.width as f32 / allocated_scratch.width as f32,
                plan.scratch_size.height as f32 / allocated_scratch.height as f32,
            ];
            let composite_uv = scale_uv_rect(
                relative_rect(physical_bounds, plan.sample_bounds),
                content_uv_scale,
            );
            let mut composite = image_uniforms(bounds, composite_uv, corner_radius, clip, scale);
            composite.sample_uv_bounds = content_texel_bounds(plan.scratch_size, allocated_scratch);
            encode_texture_quad(
                command_buffer,
                scene_texture,
                scene_size,
                MTLLoadAction::Load,
                image_pipeline(self, format),
                &self.unit_quad,
                first,
                &composite,
            )?;
            self.stats.draw_calls = self.stats.draw_calls.saturating_add(4);
            self.stats.backdrop_blur_operations =
                self.stats.backdrop_blur_operations.saturating_add(1);
            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        fn encode_blur_passes(
            &self,
            command_buffer: &metal::CommandBufferRef,
            source: &TextureRef,
            first: &TextureRef,
            second: &TextureRef,
            plan: BlurPlan,
            scene_size: SurfaceSize,
            format: MTLPixelFormat,
            transparent_edges: bool,
        ) -> Result<SurfaceSize, RenderError> {
            let allocated_scratch = texture_surface_size(first)?;
            let scratch_bounds = Rect::new(
                anmixiu_scene::Point::new(0.0, 0.0),
                anmixiu_scene::Size::new(
                    plan.scratch_size.width as f32,
                    plan.scratch_size.height as f32,
                ),
            );
            let extract_uv = normalized_rect(plan.sample_bounds, scene_size);
            let mut extract = image_uniforms(scratch_bounds, extract_uv, 0.0, None, 1.0);
            extract.sample_uv_bounds = normalized_texel_bounds(plan.sample_bounds, scene_size);
            let extract_pipeline = if plan.downsample == 1 {
                image_pipeline(self, format)
            } else {
                extract.misc = [
                    plan.downsample as f32,
                    1.0 / scene_size.width as f32,
                    1.0 / scene_size.height as f32,
                    f32::from(!rect_fills_surface(plan.sample_bounds, scene_size)),
                ];
                downsample_pipeline(self, format)
            };
            encode_texture_quad(
                command_buffer,
                first,
                plan.scratch_size,
                MTLLoadAction::Clear,
                extract_pipeline,
                &self.unit_quad,
                source,
                &extract,
            )?;

            let horizontal = blur_uniforms(
                plan.scratch_size,
                allocated_scratch,
                plan.effective_sigma,
                [1.0 / allocated_scratch.width as f32, 0.0],
            );
            encode_texture_quad(
                command_buffer,
                second,
                plan.scratch_size,
                MTLLoadAction::Clear,
                if transparent_edges {
                    filter_blur_pipeline(self, format)
                } else {
                    blur_pipeline(self, format)
                },
                &self.unit_quad,
                first,
                &horizontal,
            )?;
            let vertical = blur_uniforms(
                plan.scratch_size,
                allocated_scratch,
                plan.effective_sigma,
                [0.0, 1.0 / allocated_scratch.height as f32],
            );
            encode_texture_quad(
                command_buffer,
                first,
                plan.scratch_size,
                MTLLoadAction::Clear,
                if transparent_edges {
                    filter_blur_pipeline(self, format)
                } else {
                    blur_pipeline(self, format)
                },
                &self.unit_quad,
                second,
                &vertical,
            )?;

            Ok(allocated_scratch)
        }

        fn configure_primitive_encoder(
            &self,
            encoder: &metal::RenderCommandEncoderRef,
            size: SurfaceSize,
            format: MTLPixelFormat,
        ) {
            encoder.set_render_pipeline_state(primitive_pipeline(self, format));
            encoder.set_vertex_buffer(0, Some(&self.unit_quad), 0);
            let viewport = [size.width as f32, size.height as f32];
            encoder.set_vertex_bytes(2, size_of_val(&viewport) as u64, viewport.as_ptr().cast());
        }

        fn encode_primitive(
            &mut self,
            encoder: &metal::RenderCommandEncoderRef,
            command: &DrawCommand,
            scale: f32,
            format: MTLPixelFormat,
            border_pipeline_selected: &mut bool,
        ) -> Result<(), RenderError> {
            let needs_border_pipeline = matches!(command, DrawCommand::RoundedBorder { .. });
            if needs_border_pipeline != *border_pipeline_selected {
                encoder.set_render_pipeline_state(if needs_border_pipeline {
                    border_pipeline(self, format)
                } else {
                    primitive_pipeline(self, format)
                });
                *border_pipeline_selected = needs_border_pipeline;
            }
            match command {
                DrawCommand::SolidQuad {
                    bounds,
                    color,
                    clip,
                } => self.draw(encoder, *bounds, *color, 0.0, 0.0, *clip, None, scale),
                DrawCommand::RoundedQuad {
                    bounds,
                    color,
                    corner_radius,
                    clip,
                } => self.draw(
                    encoder,
                    *bounds,
                    *color,
                    *corner_radius,
                    0.0,
                    *clip,
                    None,
                    scale,
                ),
                DrawCommand::RoundedBorder {
                    bounds,
                    color,
                    corner_radius,
                    border_width,
                    clip,
                } => self.draw(
                    encoder,
                    *bounds,
                    *color,
                    *corner_radius,
                    *border_width,
                    *clip,
                    None,
                    scale,
                ),
                DrawCommand::Glyphs {
                    glyphs,
                    color,
                    clip,
                } => {
                    for glyph in glyphs.iter() {
                        self.draw(
                            encoder,
                            glyph.bounds,
                            *color,
                            0.0,
                            0.0,
                            *clip,
                            Some(glyph),
                            scale,
                        )?;
                    }
                    Ok(())
                }
                DrawCommand::BackdropBlur { .. } | DrawCommand::FilterBlur { .. } => Ok(()),
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn draw(
            &mut self,
            encoder: &metal::RenderCommandEncoderRef,
            bounds: Rect,
            color: Color,
            corner_radius: f32,
            border_width: f32,
            clip: Option<Clip>,
            glyph: Option<&Glyph>,
            scale: f32,
        ) -> Result<(), RenderError> {
            if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
                return Ok(());
            }
            let (clip_rect, clip_radius, has_clip) = clip.map_or(([0.0; 4], 0.0, 0.0), |clip| {
                (
                    rect_array(clip.bounds, scale),
                    clip.corner_radius.max(0.0) * scale,
                    1.0,
                )
            });
            let border_width = border_width
                .max(0.0)
                .min(bounds.size.width.max(0.0) / 2.0)
                .min(bounds.size.height.max(0.0) / 2.0);
            let uv_rect = glyph.map_or([border_width * scale, 0.0, 1.0, 1.0], |glyph| {
                rect_array(glyph.uv_bounds, 1.0)
            });
            let uniforms = DrawUniforms {
                color: linear_srgb_color(color),
                bounds: rect_array(bounds, scale),
                clip_rect,
                misc: [
                    corner_radius.max(0.0) * scale,
                    clip_radius,
                    f32::from(glyph.is_some()),
                    has_clip,
                ],
                uv_rect,
                sample_uv_bounds: [0.0, 0.0, 1.0, 1.0],
            };
            encoder.set_vertex_bytes(
                1,
                size_of::<DrawUniforms>() as u64,
                (&raw const uniforms).cast::<c_void>(),
            );
            encoder.set_fragment_bytes(
                1,
                size_of::<DrawUniforms>() as u64,
                (&raw const uniforms).cast::<c_void>(),
            );
            if let Some(glyph) = glyph {
                let cached =
                    self.atlas_textures
                        .get_mut(&glyph.atlas)
                        .ok_or(RenderError::MissingAtlas {
                            atlas: glyph.atlas.0,
                        })?;
                self.use_clock = self.use_clock.wrapping_add(1);
                cached.last_used = self.use_clock;
                encoder.set_fragment_texture(0, Some(&cached.texture));
            } else {
                encoder.set_fragment_texture(0, None);
            }
            encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 6);
            self.stats.draw_calls += 1;
            Ok(())
        }
    }

    fn begin_render_pass<'a>(
        command_buffer: &'a metal::CommandBufferRef,
        target: &TextureRef,
        load_action: MTLLoadAction,
        clear_color: MTLClearColor,
    ) -> Result<&'a metal::RenderCommandEncoderRef, RenderError> {
        let pass = RenderPassDescriptor::new();
        let attachment = pass
            .color_attachments()
            .object_at(0)
            .ok_or(RenderError::CompositorResourcesUnavailable)?;
        attachment.set_texture(Some(target));
        attachment.set_load_action(load_action);
        attachment.set_clear_color(clear_color);
        attachment.set_store_action(MTLStoreAction::Store);
        Ok(command_buffer.new_render_command_encoder(pass))
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_texture_quad(
        command_buffer: &metal::CommandBufferRef,
        target: &TextureRef,
        size: SurfaceSize,
        load_action: MTLLoadAction,
        pipeline: &RenderPipelineState,
        unit_quad: &Buffer,
        source: &TextureRef,
        uniforms: &DrawUniforms,
    ) -> Result<(), RenderError> {
        let encoder = begin_render_pass(
            command_buffer,
            target,
            load_action,
            MTLClearColor::new(0.0, 0.0, 0.0, 0.0),
        )?;
        encoder.set_render_pipeline_state(pipeline);
        encoder.set_vertex_buffer(0, Some(unit_quad), 0);
        encoder.set_viewport(MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: f64::from(size.width),
            height: f64::from(size.height),
            znear: 0.0,
            zfar: 1.0,
        });
        let viewport = [size.width as f32, size.height as f32];
        encoder.set_vertex_bytes(2, size_of_val(&viewport) as u64, viewport.as_ptr().cast());
        encoder.set_vertex_bytes(
            1,
            size_of::<DrawUniforms>() as u64,
            std::ptr::from_ref(uniforms).cast::<c_void>(),
        );
        encoder.set_fragment_bytes(
            1,
            size_of::<DrawUniforms>() as u64,
            std::ptr::from_ref(uniforms).cast::<c_void>(),
        );
        encoder.set_fragment_texture(0, Some(source));
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 6);
        encoder.end_encoding();
        Ok(())
    }

    fn valid_scale(scale: f32) -> f32 {
        if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        }
    }

    fn linear_srgb_color(color: Color) -> [f32; 4] {
        [
            linear_srgb_channel(color.r),
            linear_srgb_channel(color.g),
            linear_srgb_channel(color.b),
            color.a,
        ]
    }

    fn linear_srgb_channel(channel: f32) -> f32 {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    fn primitive_pipeline(
        renderer: &MetalRenderer,
        format: MTLPixelFormat,
    ) -> &RenderPipelineState {
        if format == MTLPixelFormat::RGBA8Unorm_sRGB {
            &renderer.rgba_pipeline
        } else {
            &renderer.bgra_pipeline
        }
    }

    fn border_pipeline(renderer: &MetalRenderer, format: MTLPixelFormat) -> &RenderPipelineState {
        if format == MTLPixelFormat::RGBA8Unorm_sRGB {
            &renderer.rgba_border_pipeline
        } else {
            &renderer.bgra_border_pipeline
        }
    }

    fn image_pipeline(renderer: &MetalRenderer, format: MTLPixelFormat) -> &RenderPipelineState {
        if format == MTLPixelFormat::RGBA8Unorm_sRGB {
            &renderer.rgba_image_pipeline
        } else {
            &renderer.bgra_image_pipeline
        }
    }

    fn downsample_pipeline(
        renderer: &MetalRenderer,
        format: MTLPixelFormat,
    ) -> &RenderPipelineState {
        if format == MTLPixelFormat::RGBA8Unorm_sRGB {
            &renderer.rgba_downsample_pipeline
        } else {
            &renderer.bgra_downsample_pipeline
        }
    }

    fn blur_pipeline(renderer: &MetalRenderer, format: MTLPixelFormat) -> &RenderPipelineState {
        if format == MTLPixelFormat::RGBA8Unorm_sRGB {
            &renderer.rgba_blur_pipeline
        } else {
            &renderer.bgra_blur_pipeline
        }
    }

    fn filter_composite_pipeline(
        renderer: &MetalRenderer,
        format: MTLPixelFormat,
    ) -> &RenderPipelineState {
        if format == MTLPixelFormat::RGBA8Unorm_sRGB {
            &renderer.rgba_filter_composite_pipeline
        } else {
            &renderer.bgra_filter_composite_pipeline
        }
    }

    fn filter_blur_pipeline(
        renderer: &MetalRenderer,
        format: MTLPixelFormat,
    ) -> &RenderPipelineState {
        if format == MTLPixelFormat::RGBA8Unorm_sRGB {
            &renderer.rgba_filter_blur_pipeline
        } else {
            &renderer.bgra_filter_blur_pipeline
        }
    }

    fn color_texture(
        device: &metal::DeviceRef,
        size: SurfaceSize,
        format: MTLPixelFormat,
    ) -> Texture {
        let descriptor = TextureDescriptor::new();
        descriptor.set_width(u64::from(size.width));
        descriptor.set_height(u64::from(size.height));
        descriptor.set_pixel_format(format);
        descriptor.set_storage_mode(MTLStorageMode::Private);
        descriptor.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        device.new_texture(&descriptor)
    }

    fn blur_textures(
        device: &metal::DeviceRef,
        size: SurfaceSize,
        format: MTLPixelFormat,
    ) -> BlurTextures {
        BlurTextures {
            size,
            format,
            first: color_texture(device, size, format),
            second: color_texture(device, size, format),
        }
    }

    fn composite_slot_bytes(slot: &CompositeSlot) -> usize {
        slot.scene
            .as_ref()
            .map_or(0, |texture| texture_bytes(texture))
            .saturating_add(slot.blur.as_ref().map_or(0, |textures| {
                texture_bytes(&textures.first).saturating_add(texture_bytes(&textures.second))
            }))
            .saturating_add(
                slot.filter_layers
                    .iter()
                    .map(|texture| texture_bytes(texture))
                    .sum::<usize>(),
            )
    }

    fn texture_bytes(texture: &TextureRef) -> usize {
        usize::try_from(texture.width())
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::try_from(texture.height()).unwrap_or(usize::MAX))
            .saturating_mul(4)
    }

    fn texture_surface_size(texture: &TextureRef) -> Result<SurfaceSize, RenderError> {
        let width = u32::try_from(texture.width()).map_err(|_| RenderError::SurfaceTooLarge {
            width: texture.width(),
            height: texture.height(),
        })?;
        let height = u32::try_from(texture.height()).map_err(|_| RenderError::SurfaceTooLarge {
            width: texture.width(),
            height: texture.height(),
        })?;
        SurfaceSize::new(width, height)
    }

    fn surface_bytes(size: SurfaceSize) -> usize {
        usize::try_from(size.width)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::try_from(size.height).unwrap_or(usize::MAX))
            .saturating_mul(4)
    }

    #[derive(Default)]
    struct CompositorRequirements {
        scratch_size: Option<SurfaceSize>,
        backdrop_blur_count: usize,
        filter_blur_count: usize,
        filter_layer_depth: usize,
    }

    fn compositor_requirements(
        commands: &[DrawCommand],
        surface: SurfaceSize,
        scale: f32,
    ) -> Result<CompositorRequirements, RenderError> {
        let mut requirements = CompositorRequirements::default();
        collect_compositor_requirements(commands, surface, scale, 0, &mut requirements)?;
        Ok(requirements)
    }

    fn collect_compositor_requirements(
        commands: &[DrawCommand],
        surface: SurfaceSize,
        scale: f32,
        filter_depth: usize,
        requirements: &mut CompositorRequirements,
    ) -> Result<(), RenderError> {
        for command in commands {
            match command {
                DrawCommand::BackdropBlur { .. } => {
                    requirements.backdrop_blur_count =
                        requirements.backdrop_blur_count.saturating_add(1);
                    if let Some(plan) = blur_plan(command, surface, scale) {
                        include_scratch_size(requirements, plan.scratch_size);
                    }
                }
                DrawCommand::FilterBlur {
                    sigma, commands, ..
                } => {
                    requirements.filter_blur_count =
                        requirements.filter_blur_count.saturating_add(1);
                    let plan = filter_blur_plan(*sigma, surface, scale);
                    let nested_depth = if let Some(plan) = plan {
                        let nested_depth = filter_depth.saturating_add(1);
                        if nested_depth > MAX_FILTER_BLUR_DEPTH {
                            return Err(RenderError::FilterBlurNestingTooDeep);
                        }
                        requirements.filter_layer_depth =
                            requirements.filter_layer_depth.max(nested_depth);
                        include_scratch_size(requirements, plan.scratch_size);
                        nested_depth
                    } else {
                        filter_depth
                    };
                    collect_compositor_requirements(
                        commands,
                        surface,
                        scale,
                        nested_depth,
                        requirements,
                    )?;
                }
                DrawCommand::SolidQuad { .. }
                | DrawCommand::RoundedQuad { .. }
                | DrawCommand::RoundedBorder { .. }
                | DrawCommand::Glyphs { .. } => {}
            }
        }
        Ok(())
    }

    fn include_scratch_size(requirements: &mut CompositorRequirements, scratch_size: SurfaceSize) {
        requirements.scratch_size = Some(requirements.scratch_size.map_or(
            scratch_size,
            |current| SurfaceSize {
                width: current.width.max(scratch_size.width),
                height: current.height.max(scratch_size.height),
            },
        ));
    }

    fn compositor_plan_bytes(
        surface: SurfaceSize,
        scratch: Option<SurfaceSize>,
        filter_layer_depth: usize,
    ) -> usize {
        surface_bytes(surface)
            .saturating_add(scratch.map_or(0, |scratch_size| {
                surface_bytes(scratch_size).saturating_mul(2)
            }))
            .saturating_add(surface_bytes(surface).saturating_mul(filter_layer_depth))
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn blur_plan(command: &DrawCommand, surface: SurfaceSize, scale: f32) -> Option<BlurPlan> {
        let DrawCommand::BackdropBlur {
            bounds,
            sigma,
            clip,
            ..
        } = command
        else {
            return None;
        };
        if !sigma.is_finite() || *sigma <= 0.0 {
            return None;
        }
        let sigma = sigma.min(MAX_BACKDROP_BLUR_SIGMA);
        let physical_sigma = sigma * scale;
        let surface_bounds = Rect::new(
            anmixiu_scene::Point::new(0.0, 0.0),
            anmixiu_scene::Size::new(surface.width as f32, surface.height as f32),
        );
        let element_bounds = scale_rect(*bounds, scale);
        let mut visible_bounds = element_bounds.intersection(surface_bounds)?;
        if let Some(clip) = clip {
            visible_bounds = visible_bounds.intersection(scale_rect(clip.bounds, scale))?;
        }
        let margin = physical_sigma * 3.0;
        let min_x = (visible_bounds.min_x() - margin).floor().max(0.0);
        let min_y = (visible_bounds.min_y() - margin).floor().max(0.0);
        let max_x = (visible_bounds.max_x() + margin)
            .ceil()
            .min(surface.width as f32);
        let max_y = (visible_bounds.max_y() + margin)
            .ceil()
            .min(surface.height as f32);
        if max_x <= min_x || max_y <= min_y {
            return None;
        }
        let sample_bounds = Rect::new(
            anmixiu_scene::Point::new(min_x, min_y),
            anmixiu_scene::Size::new(max_x - min_x, max_y - min_y),
        );
        downsampled_blur_plan(sample_bounds, physical_sigma)
    }

    fn filter_blur_plan(sigma: f32, surface: SurfaceSize, scale: f32) -> Option<BlurPlan> {
        if !sigma.is_finite() || sigma <= 0.0 {
            return None;
        }
        let sample_bounds = Rect::new(
            anmixiu_scene::Point::new(0.0, 0.0),
            anmixiu_scene::Size::new(surface.width as f32, surface.height as f32),
        );
        downsampled_blur_plan(sample_bounds, sigma.min(MAX_FILTER_BLUR_SIGMA) * scale)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn downsampled_blur_plan(sample_bounds: Rect, physical_sigma: f32) -> Option<BlurPlan> {
        let mut downsample = 1_u32;
        while physical_sigma / downsample as f32 > TARGET_DOWNSAMPLED_SIGMA
            && downsample < MAX_DOWNSAMPLE
        {
            downsample *= 2;
        }
        let scratch_width = (sample_bounds.size.width / downsample as f32)
            .ceil()
            .max(1.0) as u32;
        let scratch_height = (sample_bounds.size.height / downsample as f32)
            .ceil()
            .max(1.0) as u32;
        let scratch_size = SurfaceSize::new(scratch_width, scratch_height).ok()?;
        Some(BlurPlan {
            sample_bounds,
            scratch_size,
            downsample,
            effective_sigma: physical_sigma / downsample as f32,
        })
    }

    fn scale_rect(rect: Rect, scale: f32) -> Rect {
        Rect::new(
            anmixiu_scene::Point::new(rect.origin.x * scale, rect.origin.y * scale),
            anmixiu_scene::Size::new(rect.size.width * scale, rect.size.height * scale),
        )
    }

    fn normalized_rect(rect: Rect, surface: SurfaceSize) -> [f32; 4] {
        [
            rect.origin.x / surface.width as f32,
            rect.origin.y / surface.height as f32,
            rect.size.width / surface.width as f32,
            rect.size.height / surface.height as f32,
        ]
    }

    fn normalized_texel_bounds(rect: Rect, texture_size: SurfaceSize) -> [f32; 4] {
        [
            (rect.min_x() + 0.5) / texture_size.width as f32,
            (rect.min_y() + 0.5) / texture_size.height as f32,
            (rect.max_x() - 0.5) / texture_size.width as f32,
            (rect.max_y() - 0.5) / texture_size.height as f32,
        ]
    }

    fn rect_fills_surface(rect: Rect, surface: SurfaceSize) -> bool {
        rect.min_x() <= 0.0
            && rect.min_y() <= 0.0
            && rect.max_x() >= surface.width as f32
            && rect.max_y() >= surface.height as f32
    }

    fn content_texel_bounds(content_size: SurfaceSize, texture_size: SurfaceSize) -> [f32; 4] {
        [
            0.5 / texture_size.width as f32,
            0.5 / texture_size.height as f32,
            (content_size.width as f32 - 0.5) / texture_size.width as f32,
            (content_size.height as f32 - 0.5) / texture_size.height as f32,
        ]
    }

    fn relative_rect(rect: Rect, container: Rect) -> [f32; 4] {
        [
            (rect.origin.x - container.origin.x) / container.size.width,
            (rect.origin.y - container.origin.y) / container.size.height,
            rect.size.width / container.size.width,
            rect.size.height / container.size.height,
        ]
    }

    fn scale_uv_rect(rect: [f32; 4], scale: [f32; 2]) -> [f32; 4] {
        let [x, y, width, height] = rect;
        let [scale_x, scale_y] = scale;
        [x * scale_x, y * scale_y, width * scale_x, height * scale_y]
    }

    fn image_uniforms(
        bounds: Rect,
        uv_rect: [f32; 4],
        corner_radius: f32,
        clip: Option<Clip>,
        scale: f32,
    ) -> DrawUniforms {
        let (clip_rect, clip_radius, has_clip) = clip.map_or(([0.0; 4], 0.0, 0.0), |clip| {
            (
                rect_array(clip.bounds, scale),
                clip.corner_radius.max(0.0) * scale,
                1.0,
            )
        });
        DrawUniforms {
            color: [1.0; 4],
            bounds: rect_array(bounds, scale),
            clip_rect,
            misc: [corner_radius.max(0.0) * scale, clip_radius, 0.0, has_clip],
            uv_rect,
            sample_uv_bounds: [0.0, 0.0, 1.0, 1.0],
        }
    }

    fn blur_uniforms(
        content_size: SurfaceSize,
        texture_size: SurfaceSize,
        sigma: f32,
        direction: [f32; 2],
    ) -> DrawUniforms {
        DrawUniforms {
            color: [1.0; 4],
            bounds: [
                0.0,
                0.0,
                content_size.width as f32,
                content_size.height as f32,
            ],
            clip_rect: [0.0; 4],
            misc: [
                sigma,
                direction[0],
                direction[1],
                f32::from(content_size != texture_size),
            ],
            uv_rect: [
                0.0,
                0.0,
                content_size.width as f32 / texture_size.width as f32,
                content_size.height as f32 / texture_size.height as f32,
            ],
            sample_uv_bounds: content_texel_bounds(content_size, texture_size),
        }
    }

    fn rect_array(rect: Rect, scale: f32) -> [f32; 4] {
        [
            rect.origin.x * scale,
            rect.origin.y * scale,
            rect.size.width * scale,
            rect.size.height * scale,
        ]
    }

    fn renderer_pipelines(
        device: &metal::DeviceRef,
        library: &metal::LibraryRef,
        vertex: &metal::FunctionRef,
    ) -> Result<RendererPipelines, RenderError> {
        let gui = library
            .get_function("gui_fragment", None)
            .map_err(RenderError::ShaderCompilation)?;
        let border = library
            .get_function("border_fragment", None)
            .map_err(RenderError::ShaderCompilation)?;
        let image = library
            .get_function("image_fragment", None)
            .map_err(RenderError::ShaderCompilation)?;
        let downsample = library
            .get_function("downsample_fragment", None)
            .map_err(RenderError::ShaderCompilation)?;
        let blur = library
            .get_function("blur_fragment", None)
            .map_err(RenderError::ShaderCompilation)?;
        let filter_blur = library
            .get_function("filter_blur_fragment", None)
            .map_err(RenderError::ShaderCompilation)?;
        let (rgba, bgra) = pipeline_pair(device, vertex, &gui, true)?;
        let (rgba_border, bgra_border) = pipeline_pair(device, vertex, &border, true)?;
        let (rgba_image, bgra_image) = pipeline_pair(device, vertex, &image, false)?;
        let (rgba_downsample, bgra_downsample) = pipeline_pair(device, vertex, &downsample, false)?;
        let (rgba_blur, bgra_blur) = pipeline_pair(device, vertex, &blur, false)?;
        let (rgba_filter_blur, bgra_filter_blur) =
            pipeline_pair(device, vertex, &filter_blur, false)?;
        let (rgba_filter_composite, bgra_filter_composite) =
            premultiplied_pipeline_pair(device, vertex, &image)?;
        Ok(RendererPipelines {
            rgba,
            bgra,
            rgba_border,
            bgra_border,
            rgba_image,
            bgra_image,
            rgba_downsample,
            bgra_downsample,
            rgba_blur,
            bgra_blur,
            rgba_filter_blur,
            bgra_filter_blur,
            rgba_filter_composite,
            bgra_filter_composite,
        })
    }

    fn pipeline_pair(
        device: &metal::DeviceRef,
        vertex: &metal::FunctionRef,
        fragment: &metal::FunctionRef,
        blending: bool,
    ) -> Result<(RenderPipelineState, RenderPipelineState), RenderError> {
        Ok((
            pipeline(
                device,
                vertex,
                fragment,
                MTLPixelFormat::RGBA8Unorm_sRGB,
                blending.then_some(metal::MTLBlendFactor::SourceAlpha),
            )?,
            pipeline(
                device,
                vertex,
                fragment,
                MTLPixelFormat::BGRA8Unorm_sRGB,
                blending.then_some(metal::MTLBlendFactor::SourceAlpha),
            )?,
        ))
    }

    fn premultiplied_pipeline_pair(
        device: &metal::DeviceRef,
        vertex: &metal::FunctionRef,
        fragment: &metal::FunctionRef,
    ) -> Result<(RenderPipelineState, RenderPipelineState), RenderError> {
        Ok((
            pipeline(
                device,
                vertex,
                fragment,
                MTLPixelFormat::RGBA8Unorm_sRGB,
                Some(metal::MTLBlendFactor::One),
            )?,
            pipeline(
                device,
                vertex,
                fragment,
                MTLPixelFormat::BGRA8Unorm_sRGB,
                Some(metal::MTLBlendFactor::One),
            )?,
        ))
    }

    fn pipeline(
        device: &metal::DeviceRef,
        vertex: &metal::FunctionRef,
        fragment: &metal::FunctionRef,
        format: MTLPixelFormat,
        source_rgb_blend_factor: Option<metal::MTLBlendFactor>,
    ) -> Result<RenderPipelineState, RenderError> {
        let descriptor = RenderPipelineDescriptor::new();
        descriptor.set_vertex_function(Some(vertex));
        descriptor.set_fragment_function(Some(fragment));
        let attachment = descriptor
            .color_attachments()
            .object_at(0)
            .expect("Metal color attachment zero exists");
        attachment.set_pixel_format(format);
        attachment.set_blending_enabled(source_rgb_blend_factor.is_some());
        if let Some(source_rgb_blend_factor) = source_rgb_blend_factor {
            attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
            attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
            attachment.set_source_rgb_blend_factor(source_rgb_blend_factor);
            attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
            attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
            attachment
                .set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
        }
        device
            .new_render_pipeline_state(&descriptor)
            .map_err(RenderError::PipelineCreation)
    }

    #[cfg(test)]
    mod tests {
        use anmixiu_scene::{DrawCommand, Point, Rect, Scene, Size};

        use super::{
            COMPOSITOR_TEXTURE_BUDGET, SurfaceSize, compositor_plan_bytes, compositor_requirements,
        };

        #[test]
        fn oversized_compositor_plan_is_rejected_before_allocating_textures() {
            let surface = SurfaceSize::new(5_000, 5_000).unwrap();
            let scene = Scene::new(
                vec![DrawCommand::BackdropBlur {
                    bounds: Rect::new(Point::new(0.0, 0.0), Size::new(5_000.0, 5_000.0)),
                    sigma: 1.0,
                    corner_radius: 0.0,
                    clip: None,
                }],
                Vec::new(),
                Vec::new(),
            );
            let requirements = compositor_requirements(scene.commands(), surface, 1.0).unwrap();

            assert!(
                compositor_plan_bytes(
                    surface,
                    requirements.scratch_size,
                    requirements.filter_layer_depth,
                ) > COMPOSITOR_TEXTURE_BUDGET
            );
        }
    }
}

#[cfg(target_os = "macos")]
pub use platform::MetalRenderer;

#[cfg(not(target_os = "macos"))]
pub struct MetalRenderer;

#[cfg(not(target_os = "macos"))]
impl MetalRenderer {
    /// Reports that Metal is unavailable on this target.
    ///
    /// # Errors
    ///
    /// Always returns [`RenderError::UnsupportedPlatform`].
    pub fn new() -> Result<Option<Self>, RenderError> {
        Err(RenderError::UnsupportedPlatform)
    }
}
