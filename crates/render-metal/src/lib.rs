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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameOutcome {
    Presented,
    DrawableUnavailable {
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

    use anmixiu_scene::{AtlasId, AtlasUpload, Clip, Color, DrawCommand, Glyph, Rect, Scene};
    use core_graphics::geometry::CGSize;
    use metal::{
        Buffer, CommandQueue, CompileOptions, Device, MTLClearColor, MTLCommandBufferStatus,
        MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLResourceOptions,
        MTLSize, MTLStorageMode, MTLStoreAction, MTLTextureUsage, MetalDrawableRef, MetalLayerRef,
        RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState, Texture,
        TextureDescriptor, TextureRef,
    };

    use super::{
        FrameOutcome, OffscreenImage, RenderError, RenderStats, RendererConfig, SurfaceSize,
    };

    const SHADER_SOURCE: &str = r"
#include <metal_stdlib>
using namespace metal;

struct DrawUniforms {
    float4 color;
    float4 bounds;
    float4 clip_rect;
    float4 misc;
    float4 uv_rect;
};

struct VertexOut {
    float4 position [[position]];
    float2 local;
    float2 world;
    float2 uv;
};

vertex VertexOut gui_vertex(
    uint vertex_id [[vertex_id]],
    const device float2 *unit_vertices [[buffer(0)]],
    constant DrawUniforms &draw [[buffer(1)]],
    constant float2 &viewport [[buffer(2)]]) {
    float2 unit = unit_vertices[vertex_id];
    float2 world = draw.bounds.xy + unit * draw.bounds.zw;
    VertexOut out;
    out.position = float4(
        world.x / viewport.x * 2.0 - 1.0,
        1.0 - world.y / viewport.y * 2.0,
        0.0,
        1.0);
    out.local = unit * draw.bounds.zw;
    out.world = world;
    out.uv = draw.uv_rect.xy + unit * draw.uv_rect.zw;
    return out;
}

float rounded_distance(float2 point, float2 size, float radius) {
    float2 q = abs(point - size * 0.5) - (size * 0.5 - radius);
    return length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fragment float4 gui_fragment(
    VertexOut in [[stage_in]],
    constant DrawUniforms &draw [[buffer(1)]],
    texture2d<float> atlas [[texture(0)]]) {
    if (draw.misc.x > 0.0 && rounded_distance(in.local, draw.bounds.zw, draw.misc.x) > 0.0) {
        discard_fragment();
    }
    if (draw.misc.w > 0.5) {
        float2 clip_local = in.world - draw.clip_rect.xy;
        if (any(clip_local < float2(0.0)) || any(clip_local >= draw.clip_rect.zw)) {
            discard_fragment();
        }
        if (draw.misc.y > 0.0 && rounded_distance(clip_local, draw.clip_rect.zw, draw.misc.y) > 0.0) {
            discard_fragment();
        }
    }
    float4 color = draw.color;
    if (draw.misc.z > 0.5) {
        constexpr sampler glyph_sampler(coord::normalized, address::clamp_to_edge, filter::linear);
        color.a *= atlas.sample(glyph_sampler, in.uv).r;
        if (color.a <= 0.0) {
            discard_fragment();
        }
    }
    return color;
}

fragment float4 border_fragment(
    VertexOut in [[stage_in]],
    constant DrawUniforms &draw [[buffer(1)]]) {
    float distance = rounded_distance(in.local, draw.bounds.zw, draw.misc.x);
    if ((draw.misc.x > 0.0 && distance > 0.0) || distance < -draw.uv_rect.x) {
        discard_fragment();
    }
    if (draw.misc.w > 0.5) {
        float2 clip_local = in.world - draw.clip_rect.xy;
        if (any(clip_local < float2(0.0)) || any(clip_local >= draw.clip_rect.zw)) {
            discard_fragment();
        }
        if (draw.misc.y > 0.0 && rounded_distance(clip_local, draw.clip_rect.zw, draw.misc.y) > 0.0) {
            discard_fragment();
        }
    }
    return draw.color;
}
";

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
    }

    struct CachedAtlas {
        generation: u64,
        texture: Texture,
        last_used: u64,
    }

    pub struct MetalRenderer {
        device: Device,
        queue: CommandQueue,
        rgba_pipeline: RenderPipelineState,
        bgra_pipeline: RenderPipelineState,
        rgba_border_pipeline: RenderPipelineState,
        bgra_border_pipeline: RenderPipelineState,
        unit_quad: Buffer,
        atlas_capacity: usize,
        atlas_textures: HashMap<AtlasId, CachedAtlas>,
        use_clock: u64,
        stats: RenderStats,
        configured_surface: Cell<Option<SurfaceSize>>,
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
            let fragment = library
                .get_function("gui_fragment", None)
                .map_err(RenderError::ShaderCompilation)?;
            let border_fragment = library
                .get_function("border_fragment", None)
                .map_err(RenderError::ShaderCompilation)?;
            let rgba_pipeline = pipeline(&device, &vertex, &fragment, MTLPixelFormat::RGBA8Unorm)?;
            let bgra_pipeline = pipeline(&device, &vertex, &fragment, MTLPixelFormat::BGRA8Unorm)?;
            let rgba_border_pipeline = pipeline(
                &device,
                &vertex,
                &border_fragment,
                MTLPixelFormat::RGBA8Unorm,
            )?;
            let bgra_border_pipeline = pipeline(
                &device,
                &vertex,
                &border_fragment,
                MTLPixelFormat::BGRA8Unorm,
            )?;
            let queue = device.new_command_queue();
            let unit_quad = device.new_buffer_with_data(
                UNIT_QUAD.as_ptr().cast(),
                size_of_val(&UNIT_QUAD) as u64,
                MTLResourceOptions::StorageModeShared,
            );
            Ok(Some(Self {
                device,
                queue,
                rgba_pipeline,
                bgra_pipeline,
                rgba_border_pipeline,
                bgra_border_pipeline,
                unit_quad,
                atlas_capacity: config.atlas_texture_capacity,
                atlas_textures: HashMap::with_capacity(config.atlas_texture_capacity),
                use_clock: 0,
                stats: RenderStats::default(),
                configured_surface: Cell::new(None),
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
                ..self.stats
            }
        }

        /// Configures a `CAMetalLayer` for a physical drawable size and Retina scale.
        pub fn configure_layer(&self, layer: &MetalLayerRef, size: SurfaceSize, scale: f32) {
            self.configured_surface.set(Some(size));
            layer.set_device(&self.device);
            layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
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
                self.encode(scene, texture, size, scale, MTLPixelFormat::BGRA8Unorm)?;
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
            descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
            descriptor.set_storage_mode(MTLStorageMode::Shared);
            descriptor.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            let texture = self.device.new_texture(&descriptor);
            let command_buffer =
                self.encode(scene, &texture, size, scale, MTLPixelFormat::RGBA8Unorm)?;
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
        ) -> Result<metal::CommandBuffer, RenderError> {
            let pass = RenderPassDescriptor::new();
            let attachment = pass
                .color_attachments()
                .object_at(0)
                .expect("Metal color attachment zero exists");
            attachment.set_texture(Some(target));
            attachment.set_load_action(MTLLoadAction::Clear);
            attachment.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            attachment.set_store_action(MTLStoreAction::Store);
            let command_buffer = self.queue.new_command_buffer().to_owned();
            let encoder = command_buffer.new_render_command_encoder(pass);
            if format == MTLPixelFormat::RGBA8Unorm {
                encoder.set_render_pipeline_state(&self.rgba_pipeline);
            } else {
                encoder.set_render_pipeline_state(&self.bgra_pipeline);
            }
            encoder.set_vertex_buffer(0, Some(&self.unit_quad), 0);
            let viewport = [size.width as f32, size.height as f32];
            encoder.set_vertex_bytes(2, size_of_val(&viewport) as u64, viewport.as_ptr().cast());
            let scale = if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            };
            let mut border_pipeline_selected = false;
            for command in scene.commands() {
                let needs_border_pipeline = matches!(command, DrawCommand::RoundedBorder { .. });
                if needs_border_pipeline != border_pipeline_selected {
                    if format == MTLPixelFormat::RGBA8Unorm && needs_border_pipeline {
                        encoder.set_render_pipeline_state(&self.rgba_border_pipeline);
                    } else if format == MTLPixelFormat::RGBA8Unorm {
                        encoder.set_render_pipeline_state(&self.rgba_pipeline);
                    } else if needs_border_pipeline {
                        encoder.set_render_pipeline_state(&self.bgra_border_pipeline);
                    } else {
                        encoder.set_render_pipeline_state(&self.bgra_pipeline);
                    }
                    border_pipeline_selected = needs_border_pipeline;
                }
                match command {
                    DrawCommand::SolidQuad {
                        bounds,
                        color,
                        clip,
                    } => self.draw(encoder, *bounds, *color, 0.0, 0.0, *clip, None, scale)?,
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
                    )?,
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
                    )?,
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
                    }
                }
            }
            encoder.end_encoding();
            Ok(command_buffer)
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
                color: [color.r, color.g, color.b, color.a],
                bounds: rect_array(bounds, scale),
                clip_rect,
                misc: [
                    corner_radius.max(0.0) * scale,
                    clip_radius,
                    f32::from(glyph.is_some()),
                    has_clip,
                ],
                uv_rect,
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

    fn rect_array(rect: Rect, scale: f32) -> [f32; 4] {
        [
            rect.origin.x * scale,
            rect.origin.y * scale,
            rect.size.width * scale,
            rect.size.height * scale,
        ]
    }

    fn pipeline(
        device: &metal::DeviceRef,
        vertex: &metal::FunctionRef,
        fragment: &metal::FunctionRef,
        format: MTLPixelFormat,
    ) -> Result<RenderPipelineState, RenderError> {
        let descriptor = RenderPipelineDescriptor::new();
        descriptor.set_vertex_function(Some(vertex));
        descriptor.set_fragment_function(Some(fragment));
        let attachment = descriptor
            .color_attachments()
            .object_at(0)
            .expect("Metal color attachment zero exists");
        attachment.set_pixel_format(format);
        attachment.set_blending_enabled(true);
        attachment.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
        attachment.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
        attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::SourceAlpha);
        attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
        attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
        attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
        device
            .new_render_pipeline_state(&descriptor)
            .map_err(RenderError::PipelineCreation)
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
