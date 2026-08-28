//! CoreText shaping and a bounded, whole-page-invalidated R8 glyph atlas.
//!
//! The atlas key is `(PostScript font name, point size, glyph id, scale, subpixel-x phase)`.
//! Adding or replacing glyph pixels increments `generation`; consumers replace the
//! complete texture when that generation changes. The hard entry and pixel-page
//! capacities prevent warm steady-state growth.

#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]

use std::sync::Arc;

use anmixiu_scene::{AtlasId, AtlasUpload, Glyph, PixelSize, Point, Rect, Size};
use thiserror::Error;

/// The scene atlas id reserved for CoreText alpha glyphs.
pub const TEXT_ATLAS_ID: AtlasId = AtlasId::TEXT;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AtlasConfig {
    pub width: u32,
    pub height: u32,
    pub max_entries: usize,
}

impl AtlasConfig {
    #[must_use]
    pub const fn new(width: u32, height: u32, max_entries: usize) -> Self {
        Self {
            width,
            height,
            max_entries,
        }
    }
}

impl Default for AtlasConfig {
    fn default() -> Self {
        Self::new(1024, 1024, 2048)
    }
}

/// Selects where CoreText obtains the base font before applying fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontFamily {
    /// The current macOS system UI font selected by CoreText.
    SystemUi,
    /// A caller-selected font family or PostScript name.
    Named(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub family: FontFamily,
    pub size: f32,
}

impl FontSpec {
    /// Creates a named-font specification.
    #[must_use]
    pub fn new(family: impl Into<String>, size: f32) -> Self {
        Self {
            family: FontFamily::Named(family.into()),
            size,
        }
    }

    /// Creates a named-font request using the platform's default UI font size.
    #[must_use]
    pub fn named_default(family: impl Into<String>) -> Self {
        Self {
            family: FontFamily::Named(family.into()),
            size: 0.0,
        }
    }

    /// Creates a specification that follows the current macOS system UI font.
    #[must_use]
    pub const fn system_ui(size: f32) -> Self {
        Self {
            family: FontFamily::SystemUi,
            size,
        }
    }

    /// Follows the platform's default size for the system UI font role.
    #[must_use]
    pub const fn system_ui_default() -> Self {
        Self {
            family: FontFamily::SystemUi,
            size: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMetrics {
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AtlasStats {
    pub entries: usize,
    pub entry_capacity: usize,
    pub resident_bytes: usize,
    pub generation: u64,
    pub repacks: u64,
}

#[derive(Clone, Debug)]
pub struct ShapedText {
    pub metrics: TextMetrics,
    pub glyphs: Arc<[Glyph]>,
    pub fonts: Arc<[String]>,
    pub atlas_upload: Option<AtlasUpload>,
}

#[derive(Debug, Error, PartialEq)]
pub enum TextError {
    #[error("atlas width and height must both be non-zero")]
    InvalidAtlasDimensions,
    #[error("atlas max_entries must be non-zero")]
    InvalidAtlasCapacity,
    #[error("explicit font sizes and raster scale must be finite and greater than zero")]
    InvalidFontSize,
    #[error("CoreText could not create font `{family}`")]
    FontUnavailable { family: String },
    #[error(
        "glyph {glyph} ({width}x{height}) does not fit in the {atlas_width}x{atlas_height} atlas"
    )]
    GlyphTooLarge {
        glyph: u16,
        width: u32,
        height: u32,
        atlas_width: u32,
        atlas_height: u32,
    },
    #[error("text needs {required} unique glyphs but atlas capacity is {capacity}")]
    AtlasCapacityExceeded { required: usize, capacity: usize },
    #[error("CoreText is only available on macOS")]
    UnsupportedPlatform,
}

#[cfg(target_os = "macos")]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
mod platform {
    use std::collections::{HashMap, HashSet};

    use core_foundation::attributed_string::CFMutableAttributedString;
    use core_foundation::base::{CFRange, TCFType};
    use core_foundation::string::CFString;
    use core_graphics::base::kCGImageAlphaNoneSkipLast;
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::{CGContext, CGTextDrawingMode};
    use core_graphics::font::CGGlyph;
    use core_graphics::geometry::CGPoint;
    use core_text::font::{self, CTFont, kCTFontSystemFontType};
    use core_text::font_descriptor::kCTFontOrientationHorizontal;
    use core_text::line::CTLine;

    use super::{
        Arc, AtlasConfig, AtlasStats, AtlasUpload, FontFamily, FontSpec, Glyph, PixelSize, Point,
        Rect, ShapedText, Size, TEXT_ATLAS_ID, TextError, TextMetrics,
    };

    const GLYPH_PADDING: u32 = 2;

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct GlyphKey {
        font_name: String,
        point_size_bits: u32,
        scale_bits: u32,
        glyph: u16,
        subpixel_x: u8,
    }

    #[derive(Clone, Copy, Debug)]
    struct AtlasEntry {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    }

    #[derive(Debug)]
    struct GlyphBitmap {
        key: GlyphKey,
        glyph: u16,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct ShelfCursor {
        x: u32,
        y: u32,
        row_height: u32,
    }

    #[derive(Debug)]
    struct GlyphAtlas {
        config: AtlasConfig,
        entries: HashMap<GlyphKey, AtlasEntry>,
        pixels: Vec<u8>,
        cursor: ShelfCursor,
        generation: u64,
        repacks: u64,
    }

    impl GlyphAtlas {
        fn new(config: AtlasConfig) -> Result<Self, TextError> {
            if config.width == 0 || config.height == 0 {
                return Err(TextError::InvalidAtlasDimensions);
            }
            if config.max_entries == 0 {
                return Err(TextError::InvalidAtlasCapacity);
            }
            let pixel_count = (config.width as usize)
                .checked_mul(config.height as usize)
                .ok_or(TextError::InvalidAtlasDimensions)?;
            Ok(Self {
                config,
                entries: HashMap::with_capacity(config.max_entries),
                pixels: vec![0; pixel_count],
                cursor: ShelfCursor::default(),
                generation: 0,
                repacks: 0,
            })
        }

        fn prepare(&mut self, bitmaps: &[GlyphBitmap]) -> Result<bool, TextError> {
            let unique: HashSet<_> = bitmaps.iter().map(|bitmap| bitmap.key.clone()).collect();
            if unique.len() > self.config.max_entries {
                return Err(TextError::AtlasCapacityExceeded {
                    required: unique.len(),
                    capacity: self.config.max_entries,
                });
            }

            let mut missing: Vec<_> = bitmaps
                .iter()
                .filter(|bitmap| !self.entries.contains_key(&bitmap.key))
                .collect();
            if missing.is_empty() {
                return Ok(false);
            }

            if self.entries.len() + missing.len() > self.config.max_entries
                || !self.can_pack(&missing)
            {
                self.clear();
                missing = bitmaps.iter().collect();
            }
            if !self.can_pack(&missing) {
                let bitmap = missing
                    .iter()
                    .find(|bitmap| {
                        bitmap.width > self.config.width || bitmap.height > self.config.height
                    })
                    .copied()
                    .unwrap_or(missing[0]);
                return Err(TextError::GlyphTooLarge {
                    glyph: bitmap.glyph,
                    width: bitmap.width,
                    height: bitmap.height,
                    atlas_width: self.config.width,
                    atlas_height: self.config.height,
                });
            }

            for bitmap in missing {
                let entry = pack(&mut self.cursor, self.config, bitmap.width, bitmap.height)
                    .expect("packing was validated before mutation");
                copy_bitmap(
                    &bitmap.pixels,
                    bitmap.width,
                    bitmap.height,
                    &mut self.pixels,
                    self.config.width,
                    entry.x,
                    entry.y,
                );
                self.entries.insert(bitmap.key.clone(), entry);
            }
            self.generation = self.generation.wrapping_add(1).max(1);
            Ok(true)
        }

        fn can_pack(&self, bitmaps: &[&GlyphBitmap]) -> bool {
            let mut cursor = self.cursor;
            bitmaps
                .iter()
                .all(|bitmap| pack(&mut cursor, self.config, bitmap.width, bitmap.height).is_some())
        }

        fn clear(&mut self) {
            self.entries.clear();
            self.pixels.fill(0);
            self.cursor = ShelfCursor::default();
            // A clear repositions every glyph that gets repacked afterward, so any absolute atlas
            // UVs computed against the previous layout are now stale. Consumers that cache shaped
            // glyphs keyed by text (rather than re-reading `entries`) watch this counter to know
            // they must flush; incremental appends leave existing entries in place and do not bump
            // it.
            self.repacks = self.repacks.wrapping_add(1);
        }

        fn upload(&self) -> AtlasUpload {
            AtlasUpload {
                atlas: TEXT_ATLAS_ID,
                generation: self.generation,
                size: PixelSize {
                    width: self.config.width,
                    height: self.config.height,
                },
                pixels: Arc::from(self.pixels.clone()),
            }
        }
    }

    fn pack(
        cursor: &mut ShelfCursor,
        config: AtlasConfig,
        width: u32,
        height: u32,
    ) -> Option<AtlasEntry> {
        if width > config.width || height > config.height {
            return None;
        }
        if cursor.x + width > config.width {
            cursor.x = 0;
            cursor.y = cursor.y.checked_add(cursor.row_height)?;
            cursor.row_height = 0;
        }
        if cursor.y + height > config.height {
            return None;
        }
        let entry = AtlasEntry {
            x: cursor.x,
            y: cursor.y,
            width,
            height,
        };
        cursor.x += width;
        cursor.row_height = cursor.row_height.max(height);
        Some(entry)
    }

    fn copy_bitmap(
        source: &[u8],
        width: u32,
        height: u32,
        destination: &mut [u8],
        destination_width: u32,
        x: u32,
        y: u32,
    ) {
        for source_y in 0..height {
            // CoreGraphics bitmap coordinates are bottom-left; scene/Metal UVs are top-left.
            let flipped_y = height - source_y - 1;
            let source_start = (flipped_y * width) as usize;
            let destination_start = ((y + source_y) * destination_width + x) as usize;
            destination[destination_start..destination_start + width as usize]
                .copy_from_slice(&source[source_start..source_start + width as usize]);
        }
    }

    struct RunGlyph {
        key: GlyphKey,
        font: CTFont,
        glyph: CGGlyph,
        bounds: core_graphics::geometry::CGRect,
        physical_baseline_x: f32,
        physical_baseline_y: f32,
        subpixel_x: f64,
        raster: RasterGeometry,
    }

    #[derive(Clone, Copy, Debug)]
    struct RasterGeometry {
        left: i32,
        bottom: i32,
        top: i32,
        width: u32,
        height: u32,
    }

    #[derive(Debug)]
    pub struct TextSystem {
        atlas: GlyphAtlas,
        rasterized_glyphs: u64,
        // Key: exact point-size bits. Invalidation: replace on size change. Hard capacity: one.
        // The platform frame builder uses one base UI size; keeping this bounded avoids issuing
        // CTFontCreateUIFontForLanguage on every cached shape without growing for arbitrary sizes.
        cached_system_font: Option<(u32, CTFont)>,
    }

    impl TextSystem {
        /// Creates a text system with a fixed-size R8 atlas.
        ///
        /// # Errors
        ///
        /// Returns an error when either atlas dimension or its entry capacity is zero.
        pub fn new(config: AtlasConfig) -> Result<Self, TextError> {
            Ok(Self {
                atlas: GlyphAtlas::new(config)?,
                rasterized_glyphs: 0,
                cached_system_font: None,
            })
        }

        #[must_use]
        pub fn atlas_len(&self) -> usize {
            self.atlas.entries.len()
        }

        #[must_use]
        pub fn atlas_stats(&self) -> AtlasStats {
            AtlasStats {
                entries: self.atlas.entries.len(),
                entry_capacity: self.atlas.config.max_entries,
                resident_bytes: self.atlas.pixels.len(),
                generation: self.atlas.generation,
                repacks: self.atlas.repacks,
            }
        }

        /// Number of times the atlas has been cleared and repacked. Increments only when existing
        /// glyph positions are invalidated, never on an incremental append. Consumers caching
        /// absolute atlas UVs flush when this advances.
        #[must_use]
        pub fn atlas_repacks(&self) -> u64 {
            self.atlas.repacks
        }

        /// Cumulative CoreGraphics rasterizations, useful for validating atlas reuse.
        #[must_use]
        pub const fn rasterized_glyph_count(&self) -> u64 {
            self.rasterized_glyphs
        }

        /// Shapes and rasterizes a single line at scale 1.
        ///
        /// # Errors
        ///
        /// Returns an error for an invalid font, size, or exhausted atlas capacity.
        pub fn shape(
            &mut self,
            text: &str,
            origin: Point,
            font: &FontSpec,
        ) -> Result<ShapedText, TextError> {
            self.shape_scaled(text, origin, font, 1.0)
        }

        /// Shapes at logical coordinates and rasterizes glyphs at `scale`.
        ///
        /// # Errors
        ///
        /// Returns an error for an invalid font/scale or when the bounded atlas cannot fit
        /// all unique glyphs required by this line.
        #[allow(clippy::too_many_lines)]
        pub fn shape_scaled(
            &mut self,
            text: &str,
            origin: Point,
            font_spec: &FontSpec,
            scale: f32,
        ) -> Result<ShapedText, TextError> {
            let platform_default_size = font_spec.size.to_bits() == 0.0_f32.to_bits();
            if !font_spec.size.is_finite()
                || (!platform_default_size && font_spec.size <= 0.0)
                || !scale.is_finite()
                || scale <= 0.0
            {
                return Err(TextError::InvalidFontSize);
            }
            let base_font = match &font_spec.family {
                FontFamily::SystemUi => {
                    let size_key = font_spec.size.to_bits();
                    if let Some((cached_key, cached_font)) = &self.cached_system_font
                        && *cached_key == size_key
                    {
                        cached_font.clone()
                    } else {
                        let font = font::new_ui_font_for_language(
                            kCTFontSystemFontType,
                            f64::from(font_spec.size),
                            None,
                        );
                        self.cached_system_font = Some((size_key, font.clone()));
                        font
                    }
                }
                FontFamily::Named(family) => {
                    let size = if platform_default_size {
                        font::new_ui_font_for_language(kCTFontSystemFontType, 0.0, None).pt_size()
                    } else {
                        f64::from(font_spec.size)
                    };
                    font::new_from_name(family, size).map_err(|()| TextError::FontUnavailable {
                        family: family.clone(),
                    })?
                }
            };
            let mut attributed = CFMutableAttributedString::new();
            attributed.replace_str(&CFString::new(text), CFRange::init(0, 0));
            if attributed.char_len() != 0 {
                let font_key = CFString::new("NSFont");
                attributed.set_attribute(
                    CFRange::init(0, attributed.char_len()),
                    font_key.as_concrete_TypeRef(),
                    &base_font,
                );
            }
            let line = CTLine::new_with_attributed_string(attributed.as_concrete_TypeRef());
            let line_metrics = line.get_typographic_bounds();
            let metrics = TextMetrics {
                width: line_metrics.width as f32,
                height: (line_metrics.ascent + line_metrics.descent + line_metrics.leading) as f32,
                ascent: line_metrics.ascent as f32,
                descent: line_metrics.descent as f32,
                leading: line_metrics.leading as f32,
            };

            let mut fonts = Vec::new();
            let mut run_glyphs = Vec::new();
            for run in line.glyph_runs().iter() {
                let run_font = run
                    .attributes()
                    .and_then(|attributes| {
                        attributes.get(CFString::new("NSFont")).downcast::<CTFont>()
                    })
                    .unwrap_or_else(|| base_font.clone());
                let font_name = run_font.postscript_name();
                if !fonts.contains(&font_name) {
                    fonts.push(font_name.clone());
                }
                let glyphs = run.glyphs();
                let positions = run.positions();
                for (&glyph, &position) in glyphs.iter().zip(positions.iter()) {
                    let scaled_font = if scale.to_bits() == 1.0_f32.to_bits() {
                        run_font.clone()
                    } else {
                        run_font.clone_with_font_size(run_font.pt_size() * f64::from(scale))
                    };
                    let bounds = scaled_font
                        .get_bounding_rects_for_glyphs(kCTFontOrientationHorizontal, &[glyph]);
                    let physical_baseline_x = (origin.x + position.x as f32) * scale;
                    let physical_baseline_y =
                        (origin.y + metrics.ascent - position.y as f32) * scale;
                    let (subpixel_x_index, subpixel_x) =
                        horizontal_subpixel_phase(physical_baseline_x);
                    let raster = RasterGeometry::new(bounds, subpixel_x);
                    run_glyphs.push(RunGlyph {
                        key: GlyphKey {
                            font_name: font_name.clone(),
                            point_size_bits: (run_font.pt_size() as f32).to_bits(),
                            scale_bits: scale.to_bits(),
                            glyph,
                            subpixel_x: subpixel_x_index,
                        },
                        font: scaled_font,
                        glyph,
                        bounds,
                        physical_baseline_x,
                        physical_baseline_y,
                        subpixel_x,
                        raster,
                    });
                }
            }

            let mut unique_keys = HashSet::new();
            let unique_glyphs = run_glyphs
                .iter()
                .filter(|glyph| glyph.bounds.size.width > 0.0 && glyph.bounds.size.height > 0.0)
                .filter(|glyph| unique_keys.insert(glyph.key.clone()))
                .collect::<Vec<_>>();
            if unique_glyphs.len() > self.atlas.config.max_entries {
                return Err(TextError::AtlasCapacityExceeded {
                    required: unique_glyphs.len(),
                    capacity: self.atlas.config.max_entries,
                });
            }
            let mut candidates = unique_glyphs
                .iter()
                .copied()
                .filter(|glyph| !self.atlas.entries.contains_key(&glyph.key))
                .collect::<Vec<_>>();
            if self.atlas.entries.len() + candidates.len() > self.atlas.config.max_entries {
                self.atlas.clear();
                candidates.clone_from(&unique_glyphs);
            }
            let mut bitmaps = candidates
                .iter()
                .map(|glyph| rasterize(glyph))
                .collect::<Vec<_>>();
            self.rasterized_glyphs += bitmaps.len() as u64;
            let bitmap_refs = bitmaps.iter().collect::<Vec<_>>();
            if !self.atlas.entries.is_empty() && !self.atlas.can_pack(&bitmap_refs) {
                self.atlas.clear();
                bitmaps = unique_glyphs.iter().map(|glyph| rasterize(glyph)).collect();
                self.rasterized_glyphs += bitmaps.len() as u64;
            }
            let atlas_changed = self.atlas.prepare(&bitmaps)?;

            let atlas_width = self.atlas.config.width as f32;
            let atlas_height = self.atlas.config.height as f32;
            let inverse_scale = scale.recip();
            let glyphs = run_glyphs
                .iter()
                .filter_map(|glyph| {
                    let entry = self.atlas.entries.get(&glyph.key)?;
                    let pixel_width = entry.width.saturating_sub(GLYPH_PADDING * 2);
                    let pixel_height = entry.height.saturating_sub(GLYPH_PADDING * 2);
                    if pixel_width == 0 || pixel_height == 0 {
                        return None;
                    }
                    Some(Glyph {
                        bounds: Rect {
                            origin: Point {
                                x: (glyph.physical_baseline_x.floor() + glyph.raster.left as f32
                                    - GLYPH_PADDING as f32)
                                    * inverse_scale,
                                y: (glyph.physical_baseline_y.round()
                                    - glyph.raster.top as f32
                                    - GLYPH_PADDING as f32)
                                    * inverse_scale,
                            },
                            size: Size {
                                width: entry.width as f32 * inverse_scale,
                                height: entry.height as f32 * inverse_scale,
                            },
                        },
                        uv_bounds: Rect {
                            origin: Point {
                                x: entry.x as f32 / atlas_width,
                                y: entry.y as f32 / atlas_height,
                            },
                            size: Size {
                                width: entry.width as f32 / atlas_width,
                                height: entry.height as f32 / atlas_height,
                            },
                        },
                        atlas: TEXT_ATLAS_ID,
                    })
                })
                .collect::<Vec<_>>();

            Ok(ShapedText {
                metrics,
                glyphs: Arc::from(glyphs),
                fonts: Arc::from(fonts),
                atlas_upload: atlas_changed.then(|| self.atlas.upload()),
            })
        }
    }

    impl RasterGeometry {
        fn new(bounds: core_graphics::geometry::CGRect, subpixel_x: f64) -> Self {
            let left = (bounds.origin.x + subpixel_x).floor() as i32;
            let right = (bounds.origin.x + bounds.size.width + subpixel_x).ceil() as i32;
            let bottom = bounds.origin.y.floor() as i32;
            let top = (bounds.origin.y + bounds.size.height).ceil() as i32;
            Self {
                left,
                bottom,
                top,
                width: u32::try_from((right - left).max(1)).expect("positive glyph width"),
                height: u32::try_from((top - bottom).max(1)).expect("positive glyph height"),
            }
        }
    }

    fn rasterize(glyph: &RunGlyph) -> GlyphBitmap {
        let width = glyph.raster.width + GLYPH_PADDING * 2;
        let height = glyph.raster.height + GLYPH_PADDING * 2;
        // Draw into RGB32 so CoreGraphics applies the platform's current grayscale
        // font-smoothing behavior, then extract a renderer-independent A8 mask.
        let color_space = CGColorSpace::create_device_rgb();
        let bytes_per_row = width as usize * 4;
        let mut context = CGContext::create_bitmap_context(
            None,
            width as usize,
            height as usize,
            8,
            bytes_per_row,
            &color_space,
            kCGImageAlphaNoneSkipLast,
        );
        context.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);
        context.set_should_antialias(true);
        context.set_should_smooth_fonts(true);
        context.set_allows_font_subpixel_positioning(true);
        context.set_should_subpixel_position_fonts(true);
        context.set_allows_font_subpixel_quantization(false);
        context.set_should_subpixel_quantize_fonts(false);
        context.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
        let position = CGPoint::new(
            f64::from(GLYPH_PADDING) - f64::from(glyph.raster.left) + glyph.subpixel_x,
            f64::from(GLYPH_PADDING) - f64::from(glyph.raster.bottom),
        );
        glyph
            .font
            .draw_glyphs(&[glyph.glyph], &[position], context.clone());
        context.flush();
        // CoreGraphics bitmap rows use its bottom-up user-space convention, while Metal texture
        // coordinates and Anmixiu scene coordinates are top-down. Normalize once at the atlas
        // boundary so every renderer sees upright glyphs.
        let (rgb_pixels, remainder) = context.data().as_chunks::<4>();
        debug_assert!(remainder.is_empty());
        let mut pixels = rgb_pixels
            .iter()
            .map(|pixel| {
                let red = u16::from(pixel[0]);
                let green = u16::from(pixel[1]);
                let blue = u16::from(pixel[2]);
                u8::try_from((red * 11 + green * 16 + blue * 5) / 32)
                    .expect("weighted RGB average remains one byte")
            })
            .collect::<Vec<_>>();
        let row_bytes = width as usize;
        for top in 0..height as usize / 2 {
            let bottom = height as usize - 1 - top;
            for column in 0..row_bytes {
                pixels.swap(top * row_bytes + column, bottom * row_bytes + column);
            }
        }
        GlyphBitmap {
            key: glyph.key.clone(),
            glyph: glyph.glyph,
            width,
            height,
            pixels,
        }
    }

    fn horizontal_subpixel_phase(physical_x: f32) -> (u8, f64) {
        const POSITION_COUNT: f32 = 4.0;
        let fraction = physical_x - physical_x.floor();
        let index = (fraction * POSITION_COUNT).floor() as u8;
        (index, f64::from(index) / f64::from(POSITION_COUNT))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn system_font_cache_has_one_size_key_and_replaces_it_explicitly() {
            let mut text = TextSystem::new(AtlasConfig::new(128, 128, 32)).unwrap();
            assert!(text.cached_system_font.is_none());

            let font_18 = FontSpec::system_ui(18.0);
            text.shape("A", Point::default(), &font_18).unwrap();
            assert_eq!(
                text.cached_system_font.as_ref().map(|(key, _)| *key),
                Some(18.0_f32.to_bits())
            );
            text.shape("B", Point::default(), &font_18).unwrap();
            assert_eq!(
                text.cached_system_font.as_ref().map(|(key, _)| *key),
                Some(18.0_f32.to_bits())
            );

            text.shape("C", Point::default(), &FontSpec::system_ui(19.0))
                .unwrap();
            assert_eq!(
                text.cached_system_font.as_ref().map(|(key, _)| *key),
                Some(19.0_f32.to_bits()),
                "a different size replaces the one-entry cache instead of growing it"
            );
        }
    }
}

#[cfg(target_os = "macos")]
pub use platform::TextSystem;

#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub struct TextSystem;

#[cfg(not(target_os = "macos"))]
impl TextSystem {
    pub fn new(_config: AtlasConfig) -> Result<Self, TextError> {
        Err(TextError::UnsupportedPlatform)
    }
}
