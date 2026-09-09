//! Cross-platform `wgpu` renderer for immutable [`anmixiu_scene::Scene`] snapshots.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anmixiu_scene::{
    AtlasId, AtlasUpload, Clip, DrawCommand, MAX_BACKDROP_BLUR_SIGMA, MAX_FILTER_BLUR_SIGMA, Rect,
    Scene,
};
use bytemuck::{Pod, Zeroable};
use thiserror::Error;
use wgpu::util::DeviceExt;

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;
const MAX_BACKDROP_BLURS_PER_FRAME: usize = 64;
const MAX_FILTER_BLURS_PER_FRAME: usize = 64;
const MAX_FILTER_BLUR_DEPTH: usize = 8;
const COMPOSITOR_TEXTURE_BUDGET: usize = 256 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawInstance {
    color: [f32; 4],
    bounds: [f32; 4],
    clip_rect: [f32; 4],
    misc: [f32; 4],
    uv_rect: [f32; 4],
    draw_flags: [f32; 4],
    viewport: [f32; 2],
    _padding: [f32; 2],
}

struct PreparedDraw {
    instance: DrawInstance,
    atlas: Option<AtlasId>,
}

struct PreparedScene {
    draws: Vec<PreparedDraw>,
    commands: Vec<PreparedCommand>,
    scale: f32,
    filter_depth: usize,
    backdrop_blur_count: usize,
    filter_blur_count: usize,
}

enum PreparedCommand {
    Draw(u32),
    BackdropBlur {
        bounds: Rect,
        sigma: f32,
        corner_radius: f32,
        clip: Option<Clip>,
    },
    FilterBlur {
        sigma: f32,
        clip: Option<Clip>,
        commands: Vec<Self>,
    },
}

struct CachedAtlas {
    generation: u64,
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    byte_len: usize,
    last_used: u64,
}

struct RendererResources {
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: PipelineSet,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white_texture: wgpu::Texture,
    white_bind_group: wgpu::BindGroup,
}

struct PipelineSet {
    primitive: wgpu::RenderPipeline,
    image_blend: wgpu::RenderPipeline,
    image_replace: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
}

struct BoundTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

struct CompositorTextures {
    size: SurfaceSize,
    format: wgpu::TextureFormat,
    scene: BoundTexture,
    first: BoundTexture,
    second: BoundTexture,
    filter_layers: Vec<BoundTexture>,
}

fn compositor_bytes(textures: &CompositorTextures) -> usize {
    let texture_count = 3_usize.saturating_add(textures.filter_layers.len());
    usize::try_from(textures.size.width)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::try_from(textures.size.height).unwrap_or(usize::MAX))
        .saturating_mul(4)
        .saturating_mul(texture_count)
}

struct OffscreenSubmission {
    command_buffer: wgpu::CommandBuffer,
    readback: wgpu::Buffer,
    row_bytes: u32,
    padded_row_bytes: u32,
}

#[derive(Clone, Copy)]
struct DrawParameters {
    bounds: anmixiu_scene::Rect,
    color: anmixiu_scene::Color,
    corner_radius: f32,
    border_width: f32,
    clip: Option<anmixiu_scene::Clip>,
    uv_rect: [f32; 4],
    samples_atlas: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceSize {
    width: u32,
    height: u32,
}

impl SurfaceSize {
    /// Creates a non-empty physical render-target size.
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

    /// Checks that a physical surface still matches its configured size.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::SurfaceOutOfDate`] when the sizes differ.
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
pub enum FrameOutcome {
    Presented,
    DrawableUnavailable { retry_immediately: bool },
    SurfaceOutOfDate { retry_immediately: bool },
    SurfaceLost,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderStats {
    pub submitted_frames: u64,
    pub draw_calls: u64,
    pub atlas_uploads: u64,
    pub atlas_evictions: u64,
    pub cached_atlases: usize,
    pub cached_atlas_bytes: usize,
    pub composited_frames: u64,
    pub backdrop_blur_operations: u64,
    pub filter_blur_operations: u64,
    pub compositor_texture_bytes: usize,
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

    /// Returns one pixel.
    ///
    /// # Panics
    ///
    /// Panics when `(x, y)` is outside [`Self::size`].
    #[must_use]
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
    #[error("a scene may contain at most 64 backdrop blur operations")]
    TooManyBackdropBlurs,
    #[error("a scene may contain at most 64 filter blur operations")]
    TooManyFilterBlurs,
    #[error("filter blur nesting may not exceed 8 layers")]
    FilterBlurNestingTooDeep,
    #[error("compositor textures exceed the 256 MiB hard budget")]
    CompositorBudgetExceeded,
    #[error("no compatible wgpu adapter is available: {0}")]
    AdapterUnavailable(String),
    #[error("wgpu device creation failed: {0}")]
    Device(String),
    #[error("wgpu surface creation failed: {0}")]
    SurfaceCreation(String),
    #[error("the selected adapter cannot present to this surface")]
    SurfaceUnsupported,
    #[error("no window surface is attached to this renderer")]
    SurfaceNotAttached,
    #[error("wgpu rejected surface acquisition")]
    SurfaceValidation,
    #[error("wgpu device polling failed: {0}")]
    DevicePoll(String),
    #[error("wgpu readback mapping failed: {0}")]
    BufferMap(String),
    #[error("scene command is not implemented by the wgpu migration yet: {0}")]
    UnsupportedCommand(&'static str),
}

pub struct Renderer {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: PipelineSet,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    _white_texture: wgpu::Texture,
    white_bind_group: wgpu::BindGroup,
    atlas_capacity: usize,
    atlases: HashMap<AtlasId, CachedAtlas>,
    atlas_clock: u64,
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    surface_pipelines: Option<PipelineSet>,
    compositor: Option<CompositorTextures>,
    stats: RenderStats,
}

impl Renderer {
    /// Creates a renderer using the native backend for the current operating system.
    ///
    /// # Errors
    ///
    /// Returns an initialization error when no compatible adapter or device is available.
    pub fn new() -> Result<Self, RenderError> {
        Self::with_config(RendererConfig::default())
    }

    /// Creates a renderer with explicit bounded atlas-cache configuration.
    ///
    /// # Errors
    ///
    /// Returns an initialization error or [`RenderError::InvalidAtlasCapacity`].
    pub fn with_config(config: RendererConfig) -> Result<Self, RenderError> {
        if config.atlas_texture_capacity == 0 {
            return Err(RenderError::InvalidAtlasCapacity);
        }
        let (instance, adapter, device, queue) = create_device()?;
        let resources = create_renderer_resources(&device, &queue);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            shader: resources.shader,
            pipeline_layout: resources.pipeline_layout,
            pipelines: resources.pipelines,
            texture_bind_group_layout: resources.texture_bind_group_layout,
            sampler: resources.sampler,
            _white_texture: resources.white_texture,
            white_bind_group: resources.white_bind_group,
            atlas_capacity: config.atlas_texture_capacity,
            atlases: HashMap::with_capacity(config.atlas_texture_capacity),
            atlas_clock: 0,
            surface: None,
            surface_config: None,
            surface_pipelines: None,
            compositor: None,
            stats: RenderStats::default(),
        })
    }

    #[must_use]
    pub fn stats(&self) -> RenderStats {
        RenderStats {
            cached_atlases: self.atlases.len(),
            cached_atlas_bytes: self.atlases.values().map(|atlas| atlas.byte_len).sum(),
            compositor_texture_bytes: self.compositor.as_ref().map_or(0, compositor_bytes),
            ..self.stats
        }
    }

    /// Attaches a native window surface owned by the supplied handle provider.
    ///
    /// # Errors
    ///
    /// Returns a surface creation or adapter-compatibility error.
    pub fn attach_surface(
        &mut self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: SurfaceSize,
    ) -> Result<(), RenderError> {
        let surface = self
            .instance
            .create_surface(target)
            .map_err(|error| RenderError::SurfaceCreation(error.to_string()))?;
        let mut config = surface
            .get_default_config(&self.adapter, size.width, size.height)
            .ok_or(RenderError::SurfaceUnsupported)?;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 1;
        let pipelines = create_pipeline_set(
            &self.device,
            &self.shader,
            &self.pipeline_layout,
            config.format,
        );
        surface.configure(&self.device, &config);
        self.surface = Some(surface);
        self.surface_config = Some(config);
        self.surface_pipelines = Some(pipelines);
        Ok(())
    }

    /// Reconfigures the attached window surface to an exact physical size.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::SurfaceNotAttached`] before a surface is attached.
    pub fn resize_surface(&mut self, size: SurfaceSize) -> Result<(), RenderError> {
        let surface = self
            .surface
            .as_ref()
            .ok_or(RenderError::SurfaceNotAttached)?;
        let config = self
            .surface_config
            .as_mut()
            .ok_or(RenderError::SurfaceNotAttached)?;
        config.width = size.width;
        config.height = size.height;
        surface.configure(&self.device, config);
        Ok(())
    }

    /// Renders one scene into the attached native window surface.
    ///
    /// # Errors
    ///
    /// Returns a scene, surface, or rendering error.
    pub fn render_surface(
        &mut self,
        scene: &Scene,
        size: SurfaceSize,
        scale: f32,
    ) -> Result<FrameOutcome, RenderError> {
        let configured = self
            .surface_config
            .as_ref()
            .ok_or(RenderError::SurfaceNotAttached)?;
        SurfaceSize::new(configured.width, configured.height)?.matches(size)?;
        let surface_format = configured.format;
        let prepared = self.prepare_scene(scene, size, scale)?;
        if requires_compositor(&prepared) {
            self.ensure_compositor(&prepared, size, surface_format)?;
        }
        let surface = self
            .surface
            .as_ref()
            .ok_or(RenderError::SurfaceNotAttached)?;
        let surface_texture = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(FrameOutcome::DrawableUnavailable {
                    retry_immediately: false,
                });
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                return Ok(FrameOutcome::SurfaceOutOfDate {
                    retry_immediately: false,
                });
            }
            wgpu::CurrentSurfaceTexture::Lost => return Ok(FrameOutcome::SurfaceLost),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::SurfaceValidation);
            }
        };
        let pipelines = self
            .surface_pipelines
            .as_ref()
            .ok_or(RenderError::SurfaceNotAttached)?;
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let instances = prepared
            .draws
            .iter()
            .map(|draw| draw.instance)
            .collect::<Vec<_>>();
        let instance_buffer = create_instance_buffer(&self.device, &instances);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Anmixiu surface encoder"),
            });
        self.encode_prepared(
            &mut encoder,
            &view,
            &instance_buffer,
            &prepared,
            pipelines,
            size,
        )?;
        self.queue.submit([encoder.finish()]);
        self.queue.present(surface_texture);
        self.stats.submitted_frames = self.stats.submitted_frames.saturating_add(1);
        self.stats.draw_calls = self
            .stats
            .draw_calls
            .saturating_add(u64::try_from(prepared.draws.len()).unwrap_or(u64::MAX));
        self.note_compositor_stats(&prepared);
        Ok(FrameOutcome::Presented)
    }

    /// Renders and reads back one immutable scene.
    ///
    /// # Errors
    ///
    /// Returns a rendering or readback error.
    pub fn render_offscreen(
        &mut self,
        scene: &Scene,
        size: SurfaceSize,
    ) -> Result<OffscreenImage, RenderError> {
        self.render_offscreen_scaled(scene, size, 1.0)
    }

    /// Renders and reads back one immutable scene at a logical-to-physical scale.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidScale`] or a rendering/readback error.
    pub fn render_offscreen_scaled(
        &mut self,
        scene: &Scene,
        size: SurfaceSize,
        scale: f32,
    ) -> Result<OffscreenImage, RenderError> {
        let prepared = self.prepare_scene(scene, size, scale)?;
        if requires_compositor(&prepared) {
            self.ensure_compositor(&prepared, size, TARGET_FORMAT)?;
        }
        let submission = self.encode_offscreen(&prepared, size)?;
        self.queue.submit([submission.command_buffer]);
        let rgba = readback_rgba(
            &self.device,
            &submission.readback,
            size,
            submission.row_bytes,
            submission.padded_row_bytes,
        )?;
        self.stats.submitted_frames = self.stats.submitted_frames.saturating_add(1);
        self.stats.draw_calls = self
            .stats
            .draw_calls
            .saturating_add(u64::try_from(prepared.draws.len()).unwrap_or(u64::MAX));
        self.note_compositor_stats(&prepared);
        Ok(OffscreenImage { size, rgba })
    }

    fn encode_offscreen(
        &mut self,
        scene: &PreparedScene,
        size: SurfaceSize,
    ) -> Result<OffscreenSubmission, RenderError> {
        let instances = scene
            .draws
            .iter()
            .map(|draw| draw.instance)
            .collect::<Vec<_>>();
        let instance_buffer = create_instance_buffer(&self.device, &instances);
        let texture = create_offscreen_texture(&self.device, size);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let row_bytes = size.width.saturating_mul(4);
        let padded_row_bytes = align_up(row_bytes, COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = create_readback_buffer(&self.device, size.height, padded_row_bytes);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Anmixiu offscreen encoder"),
            });
        self.encode_prepared(
            &mut encoder,
            &view,
            &instance_buffer,
            scene,
            &self.pipelines,
            size,
        )?;
        copy_texture_to_readback(&mut encoder, &texture, &readback, size, padded_row_bytes);
        Ok(OffscreenSubmission {
            command_buffer: encoder.finish(),
            readback,
            row_bytes,
            padded_row_bytes,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_draws(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        instance_buffer: &wgpu::Buffer,
        draws: &[PreparedDraw],
        commands: &[PreparedCommand],
        load: wgpu::LoadOp<wgpu::Color>,
        pipeline: &wgpu::RenderPipeline,
    ) {
        let color_attachment = Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Anmixiu offscreen pass"),
            color_attachments: &[color_attachment],
            ..Default::default()
        });
        if commands.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, instance_buffer.slice(..));
        for index in commands.iter().filter_map(|command| match command {
            PreparedCommand::Draw(index) => Some(*index),
            PreparedCommand::BackdropBlur { .. } | PreparedCommand::FilterBlur { .. } => None,
        }) {
            let Some(draw) = draws.get(index as usize) else {
                continue;
            };
            let bind_group = draw
                .atlas
                .and_then(|atlas| self.atlases.get(&atlas))
                .map_or(&self.white_bind_group, |atlas| &atlas.bind_group);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..6, index..index.saturating_add(1));
        }
    }

    fn encode_prepared(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        instance_buffer: &wgpu::Buffer,
        scene: &PreparedScene,
        pipelines: &PipelineSet,
        size: SurfaceSize,
    ) -> Result<(), RenderError> {
        if !requires_compositor(scene) {
            self.encode_draws(
                encoder,
                target,
                instance_buffer,
                &scene.draws,
                &scene.commands,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                &pipelines.primitive,
            );
            return Ok(());
        }
        let compositor = self
            .compositor
            .as_ref()
            .ok_or(RenderError::CompositorBudgetExceeded)?;
        self.encode_command_sequence(
            encoder,
            &compositor.scene,
            &scene.commands,
            &scene.draws,
            instance_buffer,
            pipelines,
            compositor,
            scene.scale,
            0,
        )?;
        let final_instance = image_instance(
            full_logical_rect(size, scene.scale),
            0.0,
            None,
            [0.0, 0.0, 1.0, 1.0],
            size,
            scene.scale,
        );
        self.encode_texture_quad(
            encoder,
            target,
            &compositor.scene.bind_group,
            final_instance,
            &pipelines.image_replace,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn encode_command_sequence(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &BoundTexture,
        commands: &[PreparedCommand],
        draws: &[PreparedDraw],
        instance_buffer: &wgpu::Buffer,
        pipelines: &PipelineSet,
        compositor: &CompositorTextures,
        scale: f32,
        filter_depth: usize,
    ) -> Result<(), RenderError> {
        clear_view(encoder, &target.view);
        let mut pending = Vec::new();
        for command in commands {
            match command {
                PreparedCommand::Draw(index) => pending.push(*index),
                PreparedCommand::BackdropBlur {
                    bounds,
                    sigma,
                    corner_radius,
                    clip,
                } => {
                    self.flush_draws(
                        encoder,
                        &target.view,
                        instance_buffer,
                        draws,
                        &pending,
                        &pipelines.primitive,
                    );
                    pending.clear();
                    self.encode_blur(
                        encoder,
                        target,
                        compositor,
                        *sigma * scale,
                        false,
                        pipelines,
                    );
                    let instance = image_instance(
                        *bounds,
                        *corner_radius,
                        *clip,
                        normalized_rect(*bounds, compositor.size, scale),
                        compositor.size,
                        scale,
                    );
                    self.encode_texture_quad(
                        encoder,
                        &target.view,
                        &compositor.second.bind_group,
                        instance,
                        &pipelines.image_replace,
                        wgpu::LoadOp::Load,
                    );
                }
                PreparedCommand::FilterBlur {
                    sigma,
                    clip,
                    commands,
                } => {
                    self.flush_draws(
                        encoder,
                        &target.view,
                        instance_buffer,
                        draws,
                        &pending,
                        &pipelines.primitive,
                    );
                    pending.clear();
                    let layer = compositor
                        .filter_layers
                        .get(filter_depth)
                        .ok_or(RenderError::FilterBlurNestingTooDeep)?;
                    self.encode_command_sequence(
                        encoder,
                        layer,
                        commands,
                        draws,
                        instance_buffer,
                        pipelines,
                        compositor,
                        scale,
                        filter_depth.saturating_add(1),
                    )?;
                    self.encode_blur(encoder, layer, compositor, *sigma * scale, true, pipelines);
                    let instance = image_instance(
                        full_logical_rect(compositor.size, scale),
                        0.0,
                        *clip,
                        [0.0, 0.0, 1.0, 1.0],
                        compositor.size,
                        scale,
                    );
                    self.encode_texture_quad(
                        encoder,
                        &target.view,
                        &compositor.second.bind_group,
                        instance,
                        &pipelines.image_blend,
                        wgpu::LoadOp::Load,
                    );
                }
            }
        }
        self.flush_draws(
            encoder,
            &target.view,
            instance_buffer,
            draws,
            &pending,
            &pipelines.primitive,
        );
        Ok(())
    }

    fn flush_draws(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        instance_buffer: &wgpu::Buffer,
        draws: &[PreparedDraw],
        indices: &[u32],
        pipeline: &wgpu::RenderPipeline,
    ) {
        if indices.is_empty() {
            return;
        }
        let attachment = Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Anmixiu compositor draws"),
            color_attachments: &[attachment],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, instance_buffer.slice(..));
        for index in indices {
            if let Some(draw) = usize::try_from(*index)
                .ok()
                .and_then(|index| draws.get(index))
            {
                let bind_group = draw
                    .atlas
                    .and_then(|atlas| self.atlases.get(&atlas))
                    .map_or(&self.white_bind_group, |atlas| &atlas.bind_group);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..6, *index..index.saturating_add(1));
            }
        }
    }

    fn encode_blur(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &BoundTexture,
        compositor: &CompositorTextures,
        sigma: f32,
        transparent_edges: bool,
        pipelines: &PipelineSet,
    ) {
        let horizontal = blur_instance(
            compositor.size,
            sigma,
            transparent_edges,
            [1.0 / compositor.size.width as f32, 0.0],
        );
        self.encode_texture_quad(
            encoder,
            &compositor.first.view,
            &source.bind_group,
            horizontal,
            &pipelines.blur,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        );
        let vertical = blur_instance(
            compositor.size,
            sigma,
            transparent_edges,
            [0.0, 1.0 / compositor.size.height as f32],
        );
        self.encode_texture_quad(
            encoder,
            &compositor.second.view,
            &compositor.first.bind_group,
            vertical,
            &pipelines.blur,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        );
    }

    fn encode_texture_quad(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        source: &wgpu::BindGroup,
        instance: DrawInstance,
        pipeline: &wgpu::RenderPipeline,
        load: wgpu::LoadOp<wgpu::Color>,
    ) {
        let buffer = create_instance_buffer(&self.device, &[instance]);
        let color_attachment = Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Anmixiu compositor texture pass"),
            color_attachments: &[color_attachment],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.set_bind_group(0, source, &[]);
        pass.draw(0..6, 0..1);
    }

    fn note_compositor_stats(&mut self, scene: &PreparedScene) {
        if requires_compositor(scene) {
            self.stats.composited_frames = self.stats.composited_frames.saturating_add(1);
            self.stats.backdrop_blur_operations = self
                .stats
                .backdrop_blur_operations
                .saturating_add(u64::try_from(scene.backdrop_blur_count).unwrap_or(u64::MAX));
            self.stats.filter_blur_operations = self
                .stats
                .filter_blur_operations
                .saturating_add(u64::try_from(scene.filter_blur_count).unwrap_or(u64::MAX));
        }
    }

    fn ensure_compositor(
        &mut self,
        scene: &PreparedScene,
        size: SurfaceSize,
        format: wgpu::TextureFormat,
    ) -> Result<(), RenderError> {
        let texture_count = 3_usize.saturating_add(scene.filter_depth);
        let bytes = usize::try_from(size.width)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::try_from(size.height).unwrap_or(usize::MAX))
            .saturating_mul(4)
            .saturating_mul(texture_count);
        if bytes > COMPOSITOR_TEXTURE_BUDGET {
            return Err(RenderError::CompositorBudgetExceeded);
        }
        let reusable = self.compositor.as_ref().is_some_and(|textures| {
            textures.size == size
                && textures.format == format
                && textures.filter_layers.len() == scene.filter_depth
        });
        if reusable {
            return Ok(());
        }
        self.compositor = Some(create_compositor_textures(
            &self.device,
            &self.texture_bind_group_layout,
            &self.sampler,
            size,
            format,
            scene.filter_depth,
        ));
        Ok(())
    }

    fn prepare_scene(
        &mut self,
        scene: &Scene,
        size: SurfaceSize,
        scale: f32,
    ) -> Result<PreparedScene, RenderError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(RenderError::InvalidScale);
        }
        self.upload_atlases(scene.atlas_uploads())?;
        let prepared = prepare_scene_commands(scene, size, scale)?;
        if let Some(atlas) = prepared
            .draws
            .iter()
            .filter_map(|draw| draw.atlas)
            .find(|atlas| !self.atlases.contains_key(atlas))
        {
            return Err(RenderError::MissingAtlas { atlas: atlas.0 });
        }
        Ok(prepared)
    }

    fn upload_atlases(&mut self, uploads: &[AtlasUpload]) -> Result<(), RenderError> {
        for upload in uploads {
            let expected = usize::try_from(upload.size.width)
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::try_from(upload.size.height).unwrap_or(usize::MAX));
            if upload.pixels.len() != expected {
                return Err(RenderError::InvalidAtlasUpload {
                    atlas: upload.atlas.0,
                    generation: upload.generation,
                    expected,
                    actual: upload.pixels.len(),
                });
            }
            self.atlas_clock = self.atlas_clock.wrapping_add(1);
            if let Some(cached) = self.atlases.get_mut(&upload.atlas) {
                cached.last_used = self.atlas_clock;
                if cached.generation == upload.generation {
                    continue;
                }
            } else if self.atlases.len() == self.atlas_capacity
                && let Some(oldest) = self
                    .atlases
                    .iter()
                    .min_by_key(|(_, atlas)| atlas.last_used)
                    .map(|(id, _)| *id)
            {
                self.atlases.remove(&oldest);
                self.stats.atlas_evictions = self.stats.atlas_evictions.saturating_add(1);
            }
            let (texture, bind_group) = create_atlas_binding(
                &self.device,
                &self.queue,
                &self.texture_bind_group_layout,
                &self.sampler,
                upload.size.width,
                upload.size.height,
                &upload.pixels,
            );
            self.atlases.insert(
                upload.atlas,
                CachedAtlas {
                    generation: upload.generation,
                    _texture: texture,
                    bind_group,
                    byte_len: expected,
                    last_used: self.atlas_clock,
                },
            );
            self.stats.atlas_uploads = self.stats.atlas_uploads.saturating_add(1);
        }
        Ok(())
    }
}

fn create_instance_buffer(device: &wgpu::Device, instances: &[DrawInstance]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Anmixiu draw instances"),
        contents: bytemuck::cast_slice(instances),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn create_offscreen_texture(device: &wgpu::Device, size: SurfaceSize) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Anmixiu offscreen target"),
        size: wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn create_readback_buffer(
    device: &wgpu::Device,
    height: u32,
    padded_row_bytes: u32,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Anmixiu offscreen readback"),
        size: u64::from(padded_row_bytes) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

fn copy_texture_to_readback(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    readback: &wgpu::Buffer,
    size: SurfaceSize,
    padded_row_bytes: u32,
) {
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(size.height),
            },
        },
        wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
    );
}

fn readback_rgba(
    device: &wgpu::Device,
    readback: &wgpu::Buffer,
    size: SurfaceSize,
    row_bytes: u32,
    padded_row_bytes: u32,
) -> Result<Vec<u8>, RenderError> {
    let slice = readback.slice(..);
    let completion = Arc::new(Mutex::new(None));
    let callback_completion = Arc::clone(&completion);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let mut completion = callback_completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *completion = Some(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| RenderError::DevicePoll(error.to_string()))?;
    completion
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or_else(|| RenderError::BufferMap("mapping callback did not run".into()))?
        .map_err(|error| RenderError::BufferMap(error.to_string()))?;
    let mapped = slice
        .get_mapped_range()
        .map_err(|error| RenderError::BufferMap(error.to_string()))?;
    let row_bytes = usize::try_from(row_bytes).unwrap_or(usize::MAX);
    let padded_row_bytes = usize::try_from(padded_row_bytes).unwrap_or(usize::MAX);
    let mut rgba = Vec::with_capacity(
        row_bytes.saturating_mul(usize::try_from(size.height).unwrap_or(usize::MAX)),
    );
    for row in mapped.chunks(padded_row_bytes).take(size.height as usize) {
        let pixels = row
            .get(..row_bytes)
            .ok_or_else(|| RenderError::BufferMap("mapped row is shorter than expected".into()))?;
        rgba.extend_from_slice(pixels);
    }
    drop(mapped);
    readback.unmap();
    Ok(rgba)
}

fn create_device() -> Result<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue), RenderError>
{
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = native_backends();
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    }))
    .map_err(|error| RenderError::AdapterUnavailable(error.to_string()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Anmixiu renderer device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|error| RenderError::Device(error.to_string()))?;
    Ok((instance, adapter, device, queue))
}

fn create_renderer_resources(device: &wgpu::Device, queue: &wgpu::Queue) -> RendererResources {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Anmixiu GUI shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("gui.wgsl").into()),
    });
    let texture_bind_group_layout = create_atlas_layout(device);
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Anmixiu GUI pipeline layout"),
        bind_group_layouts: &[Some(&texture_bind_group_layout)],
        immediate_size: 0,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Anmixiu atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let (white_texture, white_bind_group) = create_atlas_binding(
        device,
        queue,
        &texture_bind_group_layout,
        &sampler,
        1,
        1,
        &[255],
    );
    let pipelines = create_pipeline_set(device, &shader, &pipeline_layout, TARGET_FORMAT);
    RendererResources {
        shader,
        pipeline_layout,
        pipelines,
        texture_bind_group_layout,
        sampler,
        white_texture,
        white_bind_group,
    }
}

fn create_atlas_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Anmixiu atlas layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn create_pipeline_set(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
) -> PipelineSet {
    PipelineSet {
        primitive: create_pipeline(
            device,
            shader,
            layout,
            format,
            "fragment_main",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        ),
        image_blend: create_pipeline(
            device,
            shader,
            layout,
            format,
            "image_fragment",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        ),
        image_replace: create_pipeline(device, shader, layout, format, "image_fragment", None),
        blur: create_pipeline(device, shader, layout, format, "blur_fragment", None),
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    fragment_entry: &'static str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Anmixiu GUI pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(draw_instance_layout())],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn draw_instance_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 7] = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 16,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 32,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 48,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 64,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 80,
            shader_location: 5,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 96,
            shader_location: 6,
        },
    ];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<DrawInstance>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &ATTRIBUTES,
    }
}

fn native_backends() -> wgpu::Backends {
    #[cfg(target_os = "macos")]
    {
        wgpu::Backends::METAL
    }
    #[cfg(target_os = "windows")]
    {
        wgpu::Backends::DX12
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        wgpu::Backends::VULKAN | wgpu::Backends::GL
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "freebsd"
    )))]
    {
        wgpu::Backends::empty()
    }
}

fn requires_compositor(scene: &PreparedScene) -> bool {
    scene.backdrop_blur_count != 0 || scene.filter_blur_count != 0
}

fn create_compositor_textures(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    size: SurfaceSize,
    format: wgpu::TextureFormat,
    filter_depth: usize,
) -> CompositorTextures {
    let make = || create_bound_texture(device, layout, sampler, size, format);
    CompositorTextures {
        size,
        format,
        scene: make(),
        first: make(),
        second: make(),
        filter_layers: (0..filter_depth).map(|_| make()).collect(),
    }
}

fn create_bound_texture(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    size: SurfaceSize,
    format: wgpu::TextureFormat,
) -> BoundTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Anmixiu compositor texture"),
        size: wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Anmixiu compositor texture binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    BoundTexture {
        _texture: texture,
        view,
        bind_group,
    }
}

fn clear_view(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let color_attachment = Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Anmixiu compositor clear"),
            color_attachments: &[color_attachment],
            ..Default::default()
        });
    }
}

fn full_logical_rect(size: SurfaceSize, scale: f32) -> Rect {
    Rect::new(
        anmixiu_scene::Point::new(0.0, 0.0),
        anmixiu_scene::Size::new(size.width as f32 / scale, size.height as f32 / scale),
    )
}

fn normalized_rect(bounds: Rect, size: SurfaceSize, scale: f32) -> [f32; 4] {
    [
        bounds.origin.x * scale / size.width as f32,
        bounds.origin.y * scale / size.height as f32,
        bounds.size.width * scale / size.width as f32,
        bounds.size.height * scale / size.height as f32,
    ]
}

fn image_instance(
    bounds: Rect,
    corner_radius: f32,
    clip: Option<Clip>,
    uv_rect: [f32; 4],
    size: SurfaceSize,
    scale: f32,
) -> DrawInstance {
    draw_instance(
        DrawParameters {
            bounds,
            color: anmixiu_scene::Color::WHITE,
            corner_radius,
            border_width: 0.0,
            clip,
            uv_rect,
            samples_atlas: false,
        },
        size,
        scale,
    )
}

fn blur_instance(
    size: SurfaceSize,
    sigma: f32,
    transparent_edges: bool,
    texel_step: [f32; 2],
) -> DrawInstance {
    DrawInstance {
        color: [1.0; 4],
        bounds: [0.0, 0.0, size.width as f32, size.height as f32],
        clip_rect: [0.0; 4],
        misc: [sigma.max(0.001), 0.0, flag(transparent_edges), 0.0],
        uv_rect: [0.0, 0.0, 1.0, 1.0],
        draw_flags: [0.0, 0.0, texel_step[0], texel_step[1]],
        viewport: [size.width as f32, size.height as f32],
        _padding: [0.0; 2],
    }
}

fn prepare_scene_commands(
    scene: &Scene,
    size: SurfaceSize,
    scale: f32,
) -> Result<PreparedScene, RenderError> {
    let mut draws = Vec::new();
    let mut backdrop_blur_count = 0;
    let mut filter_blur_count = 0;
    let mut filter_depth = 0;
    let commands = prepare_commands(
        scene.commands(),
        size,
        scale,
        0,
        &mut draws,
        &mut backdrop_blur_count,
        &mut filter_blur_count,
        &mut filter_depth,
    )?;
    if backdrop_blur_count > MAX_BACKDROP_BLURS_PER_FRAME {
        return Err(RenderError::TooManyBackdropBlurs);
    }
    if filter_blur_count > MAX_FILTER_BLURS_PER_FRAME {
        return Err(RenderError::TooManyFilterBlurs);
    }
    Ok(PreparedScene {
        draws,
        commands,
        scale,
        filter_depth,
        backdrop_blur_count,
        filter_blur_count,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn prepare_commands(
    commands: &[DrawCommand],
    size: SurfaceSize,
    scale: f32,
    depth: usize,
    draws: &mut Vec<PreparedDraw>,
    backdrop_blur_count: &mut usize,
    filter_blur_count: &mut usize,
    filter_depth: &mut usize,
) -> Result<Vec<PreparedCommand>, RenderError> {
    let mut prepared = Vec::new();
    for command in commands {
        match command {
            DrawCommand::SolidQuad {
                bounds,
                color,
                clip,
            } => push_draw(
                draws,
                &mut prepared,
                PreparedDraw {
                    instance: draw_instance(
                        DrawParameters {
                            bounds: *bounds,
                            color: *color,
                            corner_radius: 0.0,
                            border_width: 0.0,
                            clip: *clip,
                            uv_rect: [0.0, 0.0, 1.0, 1.0],
                            samples_atlas: false,
                        },
                        size,
                        scale,
                    ),
                    atlas: None,
                },
            ),
            DrawCommand::RoundedQuad {
                bounds,
                color,
                corner_radius,
                clip,
            } => push_draw(
                draws,
                &mut prepared,
                PreparedDraw {
                    instance: draw_instance(
                        DrawParameters {
                            bounds: *bounds,
                            color: *color,
                            corner_radius: *corner_radius,
                            border_width: 0.0,
                            clip: *clip,
                            uv_rect: [0.0, 0.0, 1.0, 1.0],
                            samples_atlas: false,
                        },
                        size,
                        scale,
                    ),
                    atlas: None,
                },
            ),
            DrawCommand::RoundedBorder {
                bounds,
                color,
                corner_radius,
                border_width,
                clip,
            } => push_draw(
                draws,
                &mut prepared,
                PreparedDraw {
                    instance: draw_instance(
                        DrawParameters {
                            bounds: *bounds,
                            color: *color,
                            corner_radius: *corner_radius,
                            border_width: *border_width,
                            clip: *clip,
                            uv_rect: [0.0, 0.0, 1.0, 1.0],
                            samples_atlas: false,
                        },
                        size,
                        scale,
                    ),
                    atlas: None,
                },
            ),
            DrawCommand::Glyphs {
                glyphs,
                color,
                clip,
            } => {
                for glyph in glyphs.iter() {
                    push_draw(
                        draws,
                        &mut prepared,
                        PreparedDraw {
                            instance: draw_instance(
                                DrawParameters {
                                    bounds: glyph.bounds,
                                    color: *color,
                                    corner_radius: 0.0,
                                    border_width: 0.0,
                                    clip: *clip,
                                    uv_rect: rect_values(glyph.uv_bounds),
                                    samples_atlas: true,
                                },
                                size,
                                scale,
                            ),
                            atlas: Some(glyph.atlas),
                        },
                    );
                }
            }
            DrawCommand::BackdropBlur {
                bounds,
                sigma,
                corner_radius,
                clip,
            } => {
                *backdrop_blur_count = backdrop_blur_count.saturating_add(1);
                prepared.push(PreparedCommand::BackdropBlur {
                    bounds: *bounds,
                    sigma: sigma.clamp(0.0, MAX_BACKDROP_BLUR_SIGMA),
                    corner_radius: *corner_radius,
                    clip: *clip,
                });
            }
            DrawCommand::FilterBlur {
                sigma,
                clip,
                commands,
            } => {
                let next_depth = depth.saturating_add(1);
                if next_depth > MAX_FILTER_BLUR_DEPTH {
                    return Err(RenderError::FilterBlurNestingTooDeep);
                }
                *filter_blur_count = filter_blur_count.saturating_add(1);
                *filter_depth = (*filter_depth).max(next_depth);
                prepared.push(PreparedCommand::FilterBlur {
                    sigma: sigma.clamp(0.0, MAX_FILTER_BLUR_SIGMA),
                    clip: *clip,
                    commands: prepare_commands(
                        commands,
                        size,
                        scale,
                        next_depth,
                        draws,
                        backdrop_blur_count,
                        filter_blur_count,
                        filter_depth,
                    )?,
                });
            }
        }
    }
    Ok(prepared)
}

fn push_draw(
    draws: &mut Vec<PreparedDraw>,
    commands: &mut Vec<PreparedCommand>,
    draw: PreparedDraw,
) {
    let index = u32::try_from(draws.len()).unwrap_or(u32::MAX);
    draws.push(draw);
    commands.push(PreparedCommand::Draw(index));
}

fn draw_instance(parameters: DrawParameters, size: SurfaceSize, scale: f32) -> DrawInstance {
    let (clip_rect, clip_radius, has_clip) = parameters.clip.map_or(([0.0; 4], 0.0, 0.0), |clip| {
        (
            scaled_rect_values(clip.bounds, scale),
            clip.corner_radius * scale,
            1.0,
        )
    });
    DrawInstance {
        color: [
            parameters.color.r,
            parameters.color.g,
            parameters.color.b,
            parameters.color.a,
        ],
        bounds: scaled_rect_values(parameters.bounds, scale),
        clip_rect,
        misc: [
            parameters.corner_radius * scale,
            clip_radius,
            flag(parameters.samples_atlas),
            has_clip,
        ],
        uv_rect: parameters.uv_rect,
        draw_flags: [
            parameters.border_width * scale,
            flag(parameters.border_width > 0.0),
            0.0,
            0.0,
        ],
        viewport: [size.width as f32, size.height as f32],
        _padding: [0.0; 2],
    }
}

fn rect_values(rect: Rect) -> [f32; 4] {
    [
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    ]
}

fn scaled_rect_values(rect: Rect, scale: f32) -> [f32; 4] {
    let mut values = rect_values(rect);
    for value in &mut values {
        *value *= scale;
    }
    values
}

const fn flag(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

fn create_atlas_binding(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Anmixiu R8 atlas"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Anmixiu atlas binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (texture, bind_group)
}

const fn align_up(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment).saturating_mul(alignment)
}
