#![forbid(unsafe_code)]

use std::{
    collections::HashMap,
    fmt,
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
};

/// Portable upper bound for a backdrop Gaussian sigma in logical pixels.
///
/// Renderers clamp larger finite values to this limit before choosing platform-specific kernels
/// and downsampling. The bound keeps effect work and sampling margins predictable across backends.
pub const MAX_BACKDROP_BLUR_SIGMA: f32 = 64.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    #[must_use]
    pub const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    #[must_use]
    pub fn min_x(self) -> f32 {
        self.origin.x
    }

    #[must_use]
    pub fn min_y(self) -> f32 {
        self.origin.y
    }

    #[must_use]
    pub fn max_x(self) -> f32 {
        self.origin.x + self.size.width
    }

    #[must_use]
    pub fn max_y(self) -> f32 {
        self.origin.y + self.size.height
    }

    /// Uses half-open bounds, matching pixel rectangles and sibling hit regions.
    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        point.x >= self.min_x()
            && point.y >= self.min_y()
            && point.x < self.max_x()
            && point.y < self.max_y()
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Option<Self> {
        let min_x = self.min_x().max(other.min_x());
        let min_y = self.min_y().max(other.min_y());
        let max_x = self.max_x().min(other.max_x());
        let max_y = self.max_y().min(other.max_y());
        (max_x > min_x && max_y > min_y).then(|| {
            Self::new(
                Point::new(min_x, min_y),
                Size::new(max_x - min_x, max_y - min_y),
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Self = Self {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    #[must_use]
    pub fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::rgba(r, g, b, 1.0)
    }

    #[must_use]
    pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            r: unit(r),
            g: unit(g),
            b: unit(b),
            a: unit(a),
        }
    }
}

fn unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub bounds: Rect,
    pub corner_radius: f32,
}

impl Clip {
    #[must_use]
    pub const fn rectangular(bounds: Rect) -> Self {
        Self {
            bounds,
            corner_radius: 0.0,
        }
    }

    #[must_use]
    pub fn rounded(bounds: Rect, corner_radius: f32) -> Self {
        Self {
            bounds,
            corner_radius: non_negative(corner_radius),
        }
    }

    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        if !self.bounds.contains(point) {
            return false;
        }

        let radius = self
            .corner_radius
            .min(self.bounds.size.width.max(0.0) / 2.0)
            .min(self.bounds.size.height.max(0.0) / 2.0);
        if radius == 0.0 {
            return true;
        }

        let inner_min_x = self.bounds.min_x() + radius;
        let inner_max_x = self.bounds.max_x() - radius;
        let inner_min_y = self.bounds.min_y() + radius;
        let inner_max_y = self.bounds.max_y() - radius;
        let closest_x = point.x.clamp(inner_min_x, inner_max_x);
        let closest_y = point.y.clamp(inner_min_y, inner_max_y);
        let dx = point.x - closest_x;
        let dy = point.y - closest_y;
        dx.mul_add(dx, dy * dy) <= radius * radius
    }
}

fn non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AtlasId(pub u64);

impl AtlasId {
    pub const TEXT: Self = Self(1);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

impl PixelSize {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// A complete R8 alpha atlas page. Renderers cache it by `(atlas, generation)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasUpload {
    pub atlas: AtlasId,
    pub generation: u64,
    pub size: PixelSize,
    pub pixels: Arc<[u8]>,
}

impl AtlasUpload {
    /// Creates a complete R8 alpha atlas page.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidAtlasUpload`] when the byte count is not exactly
    /// `size.width * size.height`, or when that product cannot fit in `usize`.
    pub fn new(
        atlas: AtlasId,
        generation: u64,
        size: PixelSize,
        pixels: Arc<[u8]>,
    ) -> Result<Self, InvalidAtlasUpload> {
        let expected = usize::try_from(size.width)
            .ok()
            .and_then(|width| {
                usize::try_from(size.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(usize::MAX);
        if pixels.len() != expected {
            return Err(InvalidAtlasUpload {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            atlas,
            generation,
            size,
            pixels,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidAtlasUpload {
    expected: usize,
    actual: usize,
}

impl InvalidAtlasUpload {
    #[must_use]
    pub const fn expected_bytes(self) -> usize {
        self.expected
    }

    #[must_use]
    pub const fn actual_bytes(self) -> usize {
        self.actual
    }
}

impl fmt::Display for InvalidAtlasUpload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "R8 atlas upload has {} bytes, expected {}",
            self.actual, self.expected
        )
    }
}

impl std::error::Error for InvalidAtlasUpload {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub bounds: Rect,
    /// Normalized atlas coordinates.
    pub uv_bounds: Rect,
    pub atlas: AtlasId,
}

impl Glyph {
    #[must_use]
    pub const fn new(bounds: Rect, uv_bounds: Rect, atlas: AtlasId) -> Self {
        Self {
            bounds,
            uv_bounds,
            atlas,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DrawCommand {
    SolidQuad {
        bounds: Rect,
        color: Color,
        clip: Option<Clip>,
    },
    RoundedQuad {
        bounds: Rect,
        color: Color,
        corner_radius: f32,
        clip: Option<Clip>,
    },
    /// A rounded stroke painted inside `bounds`.
    RoundedBorder {
        bounds: Rect,
        color: Color,
        corner_radius: f32,
        border_width: f32,
        clip: Option<Clip>,
    },
    /// Blurs the pixels produced by preceding commands, then replaces the backdrop inside the
    /// rounded bounds before subsequent commands are drawn.
    BackdropBlur {
        bounds: Rect,
        /// Gaussian sigma in logical pixels.
        sigma: f32,
        corner_radius: f32,
        /// Additional ancestor clip, such as a scroll viewport.
        clip: Option<Clip>,
    },
    Glyphs {
        glyphs: Arc<[Glyph]>,
        color: Color,
        clip: Option<Clip>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HitId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitRegion {
    pub id: HitId,
    pub bounds: Rect,
    pub clip: Option<Clip>,
}

impl HitRegion {
    #[must_use]
    pub const fn new(id: HitId, bounds: Rect, clip: Option<Clip>) -> Self {
        Self { id, bounds, clip }
    }

    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        self.bounds.contains(point) && self.clip.is_none_or(|clip| clip.contains(point))
    }
}

/// Immutable, shareable frame data. Hit regions are stored in paint order.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    commands: Arc<[DrawCommand]>,
    atlas_uploads: Arc<[AtlasUpload]>,
    hit_regions: Arc<[HitRegion]>,
    requires_compositing: OnceLock<bool>,
}

impl PartialEq for Scene {
    fn eq(&self, other: &Self) -> bool {
        self.commands == other.commands
            && self.atlas_uploads == other.atlas_uploads
            && self.hit_regions == other.hit_regions
    }
}

impl Scene {
    #[must_use]
    pub fn new(
        commands: Vec<DrawCommand>,
        atlas_uploads: Vec<AtlasUpload>,
        hit_regions: Vec<HitRegion>,
    ) -> Self {
        Self {
            commands: commands.into(),
            atlas_uploads: atlas_uploads.into(),
            hit_regions: hit_regions.into(),
            requires_compositing: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    /// Returns whether this scene needs an intermediate compositing surface.
    #[must_use]
    pub fn requires_compositing(&self) -> bool {
        *self.requires_compositing.get_or_init(|| {
            self.commands
                .iter()
                .any(|command| matches!(command, DrawCommand::BackdropBlur { .. }))
        })
    }

    #[must_use]
    pub fn atlas_uploads(&self) -> &[AtlasUpload] {
        &self.atlas_uploads
    }

    #[must_use]
    pub fn hit_regions(&self) -> &[HitRegion] {
        &self.hit_regions
    }

    #[must_use]
    pub fn hit_test(&self, point: Point) -> Option<HitId> {
        self.hit_regions
            .iter()
            .rev()
            .find(|region| region.contains(point))
            .map(|region| region.id)
    }
}

/// Full scene cache key. Geometry or paint changes must advance the matching revision;
/// a scale change is isolated by its exact finite positive `f32` bit pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneCacheKey {
    pub node: u64,
    pub paint_revision: u64,
    pub layout_revision: u64,
    scale_bits: u32,
}

impl SceneCacheKey {
    #[must_use]
    pub fn new(node: u64, paint_revision: u64, layout_revision: u64, scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        Self {
            node,
            paint_revision,
            layout_revision,
            scale_bits: scale.to_bits(),
        }
    }

    #[must_use]
    pub fn scale(self) -> f32 {
        f32::from_bits(self.scale_bits)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub invalidations: u64,
}

#[derive(Debug)]
struct CacheEntry {
    scene: Arc<Scene>,
    last_used: u64,
}

/// Capacity-bounded LRU. Entries are invalidated explicitly or by changing any key field.
#[derive(Debug)]
pub struct SceneCache {
    capacity: NonZeroUsize,
    entries: HashMap<SceneCacheKey, CacheEntry>,
    clock: u64,
    stats: SceneCacheStats,
}

impl SceneCache {
    #[must_use]
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            entries: HashMap::with_capacity(capacity.get()),
            clock: 0,
            stats: SceneCacheStats::default(),
        }
    }

    pub fn get_or_insert_with(
        &mut self,
        key: SceneCacheKey,
        build: impl FnOnce() -> Scene,
    ) -> Arc<Scene> {
        let last_used = self.next_clock();
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = last_used;
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Arc::clone(&entry.scene);
        }

        self.stats.misses = self.stats.misses.saturating_add(1);
        if self.entries.len() == self.capacity.get()
            && let Some(lru_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
        {
            self.entries.remove(&lru_key);
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }

        let scene = Arc::new(build());
        self.entries.insert(
            key,
            CacheEntry {
                scene: Arc::clone(&scene),
                last_used,
            },
        );
        scene
    }

    #[must_use]
    pub fn contains(&self, key: SceneCacheKey) -> bool {
        self.entries.contains_key(&key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub const fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    #[must_use]
    pub const fn stats(&self) -> SceneCacheStats {
        self.stats
    }

    pub fn invalidate(&mut self, key: SceneCacheKey) -> bool {
        let removed = self.entries.remove(&key).is_some();
        if removed {
            self.stats.invalidations = self.stats.invalidations.saturating_add(1);
        }
        removed
    }

    pub fn clear(&mut self) {
        let removed = u64::try_from(self.entries.len()).unwrap_or(u64::MAX);
        self.entries.clear();
        self.stats.invalidations = self.stats.invalidations.saturating_add(removed);
    }

    fn next_clock(&mut self) -> u64 {
        if self.clock == u64::MAX {
            let mut ages: Vec<_> = self
                .entries
                .iter()
                .map(|(key, entry)| (*key, entry.last_used))
                .collect();
            ages.sort_unstable_by_key(|(_, age)| *age);
            for (age, (key, _)) in ages.into_iter().enumerate() {
                if let Some(entry) = self.entries.get_mut(&key) {
                    entry.last_used = u64::try_from(age).unwrap_or(u64::MAX - 1);
                }
            }
            self.clock = u64::try_from(self.entries.len()).unwrap_or(u64::MAX - 1);
        }
        self.clock += 1;
        self.clock
    }
}
