//! DirectWrite shaping and a bounded, whole-page-invalidated R8 glyph atlas.
//!
//! The atlas key is `(font file identity, face index, em size, glyph id, scale,
//! subpixel phase)`. Adding glyph pixels advances `generation`; repacking clears
//! all prior UVs and advances `repacks`. Both entries and resident pixel bytes
//! have hard capacities supplied by [`AtlasConfig`].

#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

use std::sync::Arc;

use anmixiu_scene::{AtlasId, AtlasUpload, Glyph};
#[cfg(target_os = "windows")]
use anmixiu_scene::{PixelSize, Point, Rect, Size};
use thiserror::Error;

/// The scene atlas id reserved for native text alpha glyphs.
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontFamily {
    /// The current Windows-native system UI family resolved by the platform text backend.
    SystemUi,
    /// A caller-selected DirectWrite family name.
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

    /// Creates a named-font request using the current Windows UI font size.
    #[must_use]
    pub fn named_default(family: impl Into<String>) -> Self {
        Self {
            family: FontFamily::Named(family.into()),
            size: 0.0,
        }
    }

    /// Creates a request using the current Windows UI font family and an explicit size.
    #[must_use]
    pub const fn system_ui(size: f32) -> Self {
        Self {
            family: FontFamily::SystemUi,
            size,
        }
    }

    /// Follows both the current Windows UI font family and its logical size.
    #[must_use]
    pub const fn system_ui_default() -> Self {
        Self {
            family: FontFamily::SystemUi,
            size: 0.0,
        }
    }

    /// Returns whether either part of this request follows the current platform UI settings.
    #[must_use]
    pub fn uses_system_defaults(&self) -> bool {
        matches!(self.family, FontFamily::SystemUi) || self.size.to_bits() == 0.0_f32.to_bits()
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
    #[error("DirectWrite could not find font family `{family}`")]
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
    #[error("DirectWrite operation failed: {0}")]
    DirectWrite(String),
    #[error("Windows system UI font could not be resolved: {0}")]
    SystemUiFont(String),
    #[error("Windows system UI language could not be resolved: {0}")]
    SystemUiLanguage(String),
    #[error("DirectWrite is only available on Windows")]
    UnsupportedPlatform,
}

#[cfg(target_os = "windows")]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::inline_always,
    clippy::ref_as_ptr,
    clippy::too_many_lines
)]
mod platform {
    use std::{
        collections::{HashMap, HashSet, hash_map::DefaultHasher},
        ffi::c_void,
        hash::{Hash, Hasher},
        mem::{ManuallyDrop, size_of},
        slice,
        sync::{Arc, Mutex},
    };

    use windows::{
        Win32::{
            Foundation::{E_POINTER, RECT},
            Globalization::{
                GetThreadPreferredUILanguages, GetUserDefaultLocaleName, MUI_LANGUAGE_NAME,
            },
            Graphics::DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN,
                DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_GRID_FIT_MODE_DEFAULT, DWRITE_LINE_METRICS,
                DWRITE_MATRIX, DWRITE_MEASURING_MODE, DWRITE_RENDERING_MODE_NATURAL,
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, DWRITE_STRIKETHROUGH,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, DWRITE_TEXTURE_ALIASED_1x1, DWRITE_UNDERLINE,
                DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteFactory2,
                IDWriteFontCollection, IDWriteFontFace, IDWriteFontFile, IDWriteInlineObject,
                IDWritePixelSnapping_Impl, IDWriteTextRenderer, IDWriteTextRenderer_Impl,
            },
            System::SystemServices::LOCALE_NAME_MAX_LENGTH,
            UI::{
                HiDpi::SystemParametersInfoForDpi,
                WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS},
            },
        },
        core::{BOOL, Error as WindowsError, Interface, PCWSTR, PWSTR, Ref, implement},
    };

    use super::{
        AtlasConfig, AtlasStats, AtlasUpload, FontFamily, FontSpec, Glyph, PixelSize, Point, Rect,
        ShapedText, Size, TEXT_ATLAS_ID, TextError, TextMetrics,
    };

    const GLYPH_PADDING: u32 = 2;
    const LOGICAL_DPI: u32 = 96;
    const LEGACY_MESSAGE_FONT_SIZE: f32 = 12.0;
    const MODERN_UI_BODY_FONT_SIZE: f32 = 14.0;
    const UNBOUNDED_LAYOUT: f32 = 1_000_000.0;

    #[allow(clippy::needless_pass_by_value)]
    fn directwrite_error(error: WindowsError) -> TextError {
        TextError::DirectWrite(error.to_string())
    }

    #[derive(Clone, Debug, PartialEq)]
    struct SystemUiFont {
        family: String,
        size: f32,
    }

    fn family_is_available(
        font_collection: &IDWriteFontCollection,
        family: &str,
    ) -> Result<bool, TextError> {
        let wide = wide_null(family);
        let mut index = 0;
        let mut exists = BOOL::default();
        // SAFETY: `wide` is nul-terminated and both out parameters are valid for this call.
        unsafe {
            font_collection.FindFamilyName(PCWSTR(wide.as_ptr()), &raw mut index, &raw mut exists)
        }
        .map_err(directwrite_error)?;
        Ok(exists.as_bool())
    }

    fn modern_ui_font_size(message_font_size: f32) -> f32 {
        message_font_size * (MODERN_UI_BODY_FONT_SIZE / LEGACY_MESSAGE_FONT_SIZE)
    }

    fn rendering_mode_for_size(
        font_em_size: f32,
        scale: f32,
    ) -> windows::Win32::Graphics::DirectWrite::DWRITE_RENDERING_MODE {
        if font_em_size * scale < 16.0 {
            DWRITE_RENDERING_MODE_NATURAL
        } else {
            DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC
        }
    }

    fn resolve_system_ui_font(
        font_collection: &IDWriteFontCollection,
    ) -> Result<SystemUiFont, TextError> {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: u32::try_from(size_of::<NONCLIENTMETRICSW>()).map_err(|error| {
                TextError::SystemUiFont(format!("NONCLIENTMETRICSW size is invalid: {error}"))
            })?,
            ..NONCLIENTMETRICSW::default()
        };
        // SAFETY: `metrics` is a correctly sized, writable NONCLIENTMETRICSW out parameter. The
        // canonical 96-DPI query converts its LOGFONT height directly into DirectWrite DIPs.
        unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some((&raw mut metrics).cast::<c_void>()),
                0,
                LOGICAL_DPI,
            )
        }
        .map_err(|error| TextError::SystemUiFont(error.to_string()))?;

        let face_name = &metrics.lfMessageFont.lfFaceName;
        let length = face_name
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(face_name.len());
        let family = String::from_utf16(&face_name[..length])
            .map_err(|error| TextError::SystemUiFont(error.to_string()))?;
        if family.is_empty() {
            return Err(TextError::SystemUiFont(
                "NONCLIENTMETRICS returned an empty message-font family".to_owned(),
            ));
        }
        let message_font_size = metrics.lfMessageFont.lfHeight.unsigned_abs() as f32;
        let size = modern_ui_font_size(message_font_size);
        if size <= 0.0 {
            return Err(TextError::SystemUiFont(
                "NONCLIENTMETRICS returned a zero message-font height".to_owned(),
            ));
        }
        // Windows 11's modern UI uses Segoe UI Variable for Latin and relies on DirectWrite's
        // locale-aware fallback for scripts such as CJK. Older Windows versions expose Segoe UI;
        // the non-client family remains the final fallback for customized or minimal systems.
        let mut resolved_family = family;
        for candidate in ["Segoe UI Variable", "Segoe UI"] {
            if family_is_available(font_collection, candidate)? {
                candidate.clone_into(&mut resolved_family);
                break;
            }
        }
        Ok(SystemUiFont {
            family: resolved_family,
            size,
        })
    }

    fn first_language_name(buffer: &[u16]) -> Result<String, String> {
        let length = buffer
            .iter()
            .position(|character| *character == 0)
            .ok_or_else(|| "Windows returned a language buffer without a terminator".to_owned())?;
        if length == 0 {
            return Err("Windows returned an empty language name".to_owned());
        }
        String::from_utf16(&buffer[..length]).map_err(|error| error.to_string())
    }

    fn resolve_thread_ui_language() -> Result<String, String> {
        let mut language_count = 0;
        let mut buffer_length = 0;
        // SAFETY: Null output requests the required multistring length; both counters are valid
        // writable out parameters.
        unsafe {
            GetThreadPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &raw mut language_count,
                None,
                &raw mut buffer_length,
            )
        }
        .map_err(|error| error.to_string())?;
        let capacity = usize::try_from(buffer_length)
            .map_err(|error| format!("preferred-language buffer length is invalid: {error}"))?;
        if capacity == 0 || language_count == 0 {
            return Err("Windows returned no preferred thread UI languages".to_owned());
        }
        let mut buffer = vec![0_u16; capacity];
        // SAFETY: `buffer` has the exact UTF-16 capacity requested above and remains writable for
        // the duration of the call; both counters are valid in/out parameters.
        unsafe {
            GetThreadPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &raw mut language_count,
                Some(PWSTR(buffer.as_mut_ptr())),
                &raw mut buffer_length,
            )
        }
        .map_err(|error| error.to_string())?;
        first_language_name(&buffer)
    }

    fn resolve_user_locale() -> Result<String, String> {
        let capacity = usize::try_from(LOCALE_NAME_MAX_LENGTH)
            .map_err(|error| format!("locale-name capacity is invalid: {error}"))?;
        let mut buffer = vec![0_u16; capacity];
        // SAFETY: `buffer` is writable and has Windows' documented maximum locale-name capacity.
        let length = unsafe { GetUserDefaultLocaleName(&mut buffer) };
        if length == 0 {
            return Err(WindowsError::from_thread().to_string());
        }
        first_language_name(&buffer)
    }

    fn resolve_system_ui_language() -> Result<String, TextError> {
        match resolve_thread_ui_language() {
            Ok(language) => Ok(language),
            Err(thread_error) => resolve_user_locale().map_err(|locale_error| {
                TextError::SystemUiLanguage(format!(
                    "preferred thread UI language failed ({thread_error}); user locale fallback failed ({locale_error})"
                ))
            }),
        }
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct GlyphKey {
        font_identity: u64,
        em_size_bits: u32,
        scale_bits: u32,
        glyph: u16,
        subpixel_x: u8,
        subpixel_y: u8,
    }

    #[derive(Clone, Copy, Debug)]
    struct AtlasEntry {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct ShelfCursor {
        x: u32,
        y: u32,
        row_height: u32,
    }

    #[derive(Debug)]
    struct GlyphBitmap {
        key: GlyphKey,
        glyph: u16,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
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
            let pixel_count = usize::try_from(config.width)
                .ok()
                .and_then(|width| {
                    usize::try_from(config.height)
                        .ok()
                        .and_then(|height| width.checked_mul(height))
                })
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

        fn needs_repack(&self, glyphs: &[RasterGlyph]) -> bool {
            let missing = glyphs
                .iter()
                .filter(|glyph| !self.entries.contains_key(&glyph.key))
                .collect::<Vec<_>>();
            self.entries.len() + missing.len() > self.config.max_entries
                || !self.can_pack(missing.into_iter())
        }

        fn can_pack<'a>(&self, mut glyphs: impl Iterator<Item = &'a RasterGlyph>) -> bool {
            let mut cursor = self.cursor;
            glyphs.all(|glyph| {
                reserve(
                    &mut cursor,
                    glyph.padded_width,
                    glyph.padded_height,
                    self.config,
                )
                .is_some()
            })
        }

        fn clear_for_repack(&mut self) {
            if !self.entries.is_empty() {
                self.repacks = self.repacks.saturating_add(1);
            }
            self.entries.clear();
            self.pixels.fill(0);
            self.cursor = ShelfCursor::default();
        }

        fn insert(&mut self, bitmaps: &[GlyphBitmap]) -> Result<bool, TextError> {
            let mut changed = false;
            for bitmap in bitmaps {
                if self.entries.contains_key(&bitmap.key) {
                    continue;
                }
                let Some((x, y)) =
                    reserve(&mut self.cursor, bitmap.width, bitmap.height, self.config)
                else {
                    return Err(TextError::GlyphTooLarge {
                        glyph: bitmap.glyph,
                        width: bitmap.width,
                        height: bitmap.height,
                        atlas_width: self.config.width,
                        atlas_height: self.config.height,
                    });
                };
                for (row, source) in bitmap
                    .pixels
                    .chunks_exact(usize::try_from(bitmap.width).unwrap_or(usize::MAX))
                    .enumerate()
                {
                    let row = u32::try_from(row).unwrap_or(u32::MAX);
                    let start = usize::try_from((y + row) * self.config.width + x)
                        .map_err(|_| TextError::InvalidAtlasDimensions)?;
                    let end = start
                        .checked_add(source.len())
                        .ok_or(TextError::InvalidAtlasDimensions)?;
                    let Some(target) = self.pixels.get_mut(start..end) else {
                        return Err(TextError::InvalidAtlasDimensions);
                    };
                    target.copy_from_slice(source);
                }
                self.entries.insert(
                    bitmap.key.clone(),
                    AtlasEntry {
                        x,
                        y,
                        width: bitmap.width,
                        height: bitmap.height,
                    },
                );
                changed = true;
            }
            if changed {
                self.generation = self.generation.saturating_add(1);
            }
            Ok(changed)
        }

        fn upload(&self) -> AtlasUpload {
            AtlasUpload {
                atlas: TEXT_ATLAS_ID,
                generation: self.generation,
                size: PixelSize::new(self.config.width, self.config.height),
                pixels: Arc::from(self.pixels.clone()),
            }
        }
    }

    fn reserve(
        cursor: &mut ShelfCursor,
        width: u32,
        height: u32,
        config: AtlasConfig,
    ) -> Option<(u32, u32)> {
        if width > config.width || height > config.height {
            return None;
        }
        if cursor.x.checked_add(width)? > config.width {
            cursor.x = 0;
            cursor.y = cursor.y.checked_add(cursor.row_height)?;
            cursor.row_height = 0;
        }
        if cursor.y.checked_add(height)? > config.height {
            return None;
        }
        let position = (cursor.x, cursor.y);
        cursor.x = cursor.x.checked_add(width)?;
        cursor.row_height = cursor.row_height.max(height);
        Some(position)
    }

    #[derive(Debug)]
    struct CollectedRun {
        baseline_x: f32,
        baseline_y: f32,
        measuring_mode: DWRITE_MEASURING_MODE,
        font_face: IDWriteFontFace,
        font_em_size: f32,
        glyph_indices: Vec<u16>,
        glyph_advances: Vec<f32>,
        glyph_offsets: Vec<DWRITE_GLYPH_OFFSET>,
        is_sideways: BOOL,
        bidi_level: u32,
    }

    #[implement(IDWriteTextRenderer)]
    struct RunCollector {
        runs: Arc<Mutex<Vec<CollectedRun>>>,
        scale: f32,
    }

    impl IDWritePixelSnapping_Impl for RunCollector_Impl {
        fn IsPixelSnappingDisabled(&self, _context: *const c_void) -> windows::core::Result<BOOL> {
            Ok(false.into())
        }

        fn GetCurrentTransform(
            &self,
            _context: *const c_void,
            transform: *mut DWRITE_MATRIX,
        ) -> windows::core::Result<()> {
            if transform.is_null() {
                return Err(WindowsError::from(E_POINTER));
            }
            // SAFETY: DirectWrite supplies a writable transform out-pointer for this synchronous
            // callback; the null case is rejected above.
            unsafe {
                transform.write(DWRITE_MATRIX {
                    m11: 1.0,
                    m12: 0.0,
                    m21: 0.0,
                    m22: 1.0,
                    dx: 0.0,
                    dy: 0.0,
                });
            }
            Ok(())
        }

        fn GetPixelsPerDip(&self, _context: *const c_void) -> windows::core::Result<f32> {
            Ok(self.scale)
        }
    }

    impl IDWriteTextRenderer_Impl for RunCollector_Impl {
        fn DrawGlyphRun(
            &self,
            _context: *const c_void,
            baseline_x: f32,
            baseline_y: f32,
            measuring_mode: DWRITE_MEASURING_MODE,
            glyph_run: *const DWRITE_GLYPH_RUN,
            _description: *const DWRITE_GLYPH_RUN_DESCRIPTION,
            _effect: Ref<'_, windows::core::IUnknown>,
        ) -> windows::core::Result<()> {
            if glyph_run.is_null() {
                return Err(WindowsError::from(E_POINTER));
            }
            // SAFETY: DirectWrite keeps the glyph run and all arrays valid for the duration of
            // this synchronous callback. Every array is copied before the callback returns.
            let run = unsafe { &*glyph_run };
            let count =
                usize::try_from(run.glyphCount).map_err(|_| WindowsError::from(E_POINTER))?;
            if count == 0 {
                return Ok(());
            }
            let Some(font_face) = run.fontFace.as_ref().cloned() else {
                return Err(WindowsError::from(E_POINTER));
            };
            if run.glyphIndices.is_null() {
                return Err(WindowsError::from(E_POINTER));
            }
            // SAFETY: `glyphIndices` has `glyphCount` entries by the DWRITE_GLYPH_RUN contract.
            let glyph_indices = unsafe { slice::from_raw_parts(run.glyphIndices, count) }.to_vec();
            let glyph_advances = if run.glyphAdvances.is_null() {
                vec![0.0; count]
            } else {
                // SAFETY: A non-null `glyphAdvances` array has `glyphCount` entries.
                unsafe { slice::from_raw_parts(run.glyphAdvances, count) }.to_vec()
            };
            let glyph_offsets = if run.glyphOffsets.is_null() {
                vec![DWRITE_GLYPH_OFFSET::default(); count]
            } else {
                // SAFETY: A non-null `glyphOffsets` array has `glyphCount` entries.
                unsafe { slice::from_raw_parts(run.glyphOffsets, count) }.to_vec()
            };
            self.runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(CollectedRun {
                    baseline_x,
                    baseline_y,
                    measuring_mode,
                    font_face,
                    font_em_size: run.fontEmSize,
                    glyph_indices,
                    glyph_advances,
                    glyph_offsets,
                    is_sideways: run.isSideways,
                    bidi_level: run.bidiLevel,
                });
            Ok(())
        }

        fn DrawUnderline(
            &self,
            _context: *const c_void,
            _baseline_x: f32,
            _baseline_y: f32,
            _underline: *const DWRITE_UNDERLINE,
            _effect: Ref<'_, windows::core::IUnknown>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn DrawStrikethrough(
            &self,
            _context: *const c_void,
            _baseline_x: f32,
            _baseline_y: f32,
            _strikethrough: *const DWRITE_STRIKETHROUGH,
            _effect: Ref<'_, windows::core::IUnknown>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn DrawInlineObject(
            &self,
            _context: *const c_void,
            _origin_x: f32,
            _origin_y: f32,
            _inline_object: Ref<'_, IDWriteInlineObject>,
            _is_sideways: BOOL,
            _is_right_to_left: BOOL,
            _effect: Ref<'_, windows::core::IUnknown>,
        ) -> windows::core::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct RasterGlyph {
        key: GlyphKey,
        glyph: u16,
        analysis: windows::Win32::Graphics::DirectWrite::IDWriteGlyphRunAnalysis,
        bounds: RECT,
        padded_width: u32,
        padded_height: u32,
    }

    #[derive(Debug)]
    pub struct TextSystem {
        factory: IDWriteFactory,
        analysis_factory: IDWriteFactory2,
        font_collection: IDWriteFontCollection,
        system_ui_font: SystemUiFont,
        system_ui_language: String,
        atlas: GlyphAtlas,
        rasterized_glyphs: u64,
    }

    impl TextSystem {
        /// Creates a DirectWrite text system with a bounded alpha atlas.
        ///
        /// # Errors
        ///
        /// Returns an error when the atlas configuration is invalid or DirectWrite cannot
        /// initialize its shared factory and system font collection.
        pub fn new(config: AtlasConfig) -> Result<Self, TextError> {
            let atlas = GlyphAtlas::new(config)?;
            // SAFETY: DWriteCreateFactory initializes a process-shared DirectWrite factory and
            // the returned COM interface is managed by `windows` reference counting.
            let factory: IDWriteFactory =
                unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
                    .map_err(directwrite_error)?;
            let analysis_factory = factory
                .cast::<IDWriteFactory2>()
                .map_err(directwrite_error)?;
            let mut font_collection = None;
            // SAFETY: `font_collection` is a valid out parameter and `false` avoids a blocking
            // system-font rescan on this UI-facing initialization path.
            unsafe { factory.GetSystemFontCollection(&raw mut font_collection, false) }
                .map_err(directwrite_error)?;
            let font_collection = font_collection.ok_or_else(|| {
                TextError::DirectWrite("system font collection was not returned".to_owned())
            })?;
            let system_ui_font = resolve_system_ui_font(&font_collection)?;
            let system_ui_language = resolve_system_ui_language()?;
            Ok(Self {
                factory,
                analysis_factory,
                font_collection,
                system_ui_font,
                system_ui_language,
                atlas,
                rasterized_glyphs: 0,
            })
        }

        /// Re-reads the current Windows UI font and effective text language.
        ///
        /// Returns whether any setting changed. Callers retaining shaped text or layout results
        /// must invalidate those caches when this returns `true`; language changes can affect
        /// shaping and font fallback even when the requested font is explicit.
        ///
        /// # Errors
        ///
        /// Returns a structured error when Windows does not provide valid font or language data.
        pub fn refresh_system_text_settings(&mut self) -> Result<bool, TextError> {
            let current_font = resolve_system_ui_font(&self.font_collection)?;
            let current_language = resolve_system_ui_language()?;
            if current_font == self.system_ui_font && current_language == self.system_ui_language {
                return Ok(false);
            }
            self.system_ui_font = current_font;
            self.system_ui_language = current_language;
            Ok(true)
        }

        /// Compatibility entry point for callers that previously refreshed only the UI font.
        ///
        /// The refresh now also observes the effective Windows UI language because it can change
        /// shaping and fallback. The return value is `true` when either setting changed.
        ///
        /// # Errors
        ///
        /// Returns a structured error when Windows does not provide valid font or language data.
        pub fn refresh_system_ui_font(&mut self) -> Result<bool, TextError> {
            self.refresh_system_text_settings()
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

        #[must_use]
        pub const fn atlas_repacks(&self) -> u64 {
            self.atlas.repacks
        }

        #[must_use]
        pub const fn rasterized_glyph_count(&self) -> u64 {
            self.rasterized_glyphs
        }

        /// Shapes and rasterizes a single unwrapped UI-text line at 1x scale.
        ///
        /// # Errors
        ///
        /// Returns an error for an invalid font, an exhausted atlas, a glyph that cannot fit,
        /// or a failed DirectWrite operation.
        pub fn shape(
            &mut self,
            text: &str,
            origin: Point,
            font: &FontSpec,
        ) -> Result<ShapedText, TextError> {
            self.shape_scaled(text, origin, font, 1.0)
        }

        /// Shapes and rasterizes a single unwrapped UI-text line for a display scale.
        ///
        /// # Errors
        ///
        /// Returns an error for an invalid font or scale, an exhausted atlas, a glyph that
        /// cannot fit, or a failed DirectWrite operation.
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
            let family = match &font_spec.family {
                FontFamily::SystemUi => &self.system_ui_font.family,
                FontFamily::Named(family) => {
                    self.validate_family(family)?;
                    family
                }
            };
            let size = if platform_default_size {
                self.system_ui_font.size
            } else {
                font_spec.size
            };
            let family_wide = wide_null(family);
            let locale_wide = wide_null(&self.system_ui_language);
            // SAFETY: Both UTF-16 buffers are nul-terminated for the duration of the call; the
            // system font collection and factory outlive the returned format.
            let format = unsafe {
                self.factory.CreateTextFormat(
                    PCWSTR(family_wide.as_ptr()),
                    &self.font_collection,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    PCWSTR(locale_wide.as_ptr()),
                )
            }
            .map_err(directwrite_error)?;
            // SAFETY: `format` is a live DirectWrite text format created immediately above.
            unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) }
                .map_err(directwrite_error)?;
            let value = text.encode_utf16().collect::<Vec<_>>();
            // SAFETY: DirectWrite consumes the UTF-16 slice during the call and retains its own
            // layout data. The finite bounds intentionally disable wrapping for one-line UI text.
            let layout = unsafe {
                self.factory
                    .CreateTextLayout(&value, &format, UNBOUNDED_LAYOUT, UNBOUNDED_LAYOUT)
            }
            .map_err(directwrite_error)?;

            let mut raw_metrics =
                windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
            // SAFETY: `raw_metrics` is a valid writable out parameter.
            unsafe { layout.GetMetrics(&raw mut raw_metrics) }.map_err(directwrite_error)?;
            let mut line_count = raw_metrics.lineCount;
            let mut lines = vec![
                DWRITE_LINE_METRICS::default();
                usize::try_from(line_count).unwrap_or(usize::MAX)
            ];
            if !lines.is_empty() {
                // SAFETY: `lines` has the exact capacity returned by the preceding query.
                unsafe { layout.GetLineMetrics(Some(&mut lines), &raw mut line_count) }
                    .map_err(directwrite_error)?;
            }
            let first_line = lines.first().copied().unwrap_or_default();
            let ascent = first_line.baseline;
            let descent = (first_line.height - first_line.baseline).max(0.0);
            let metrics = TextMetrics {
                width: raw_metrics.widthIncludingTrailingWhitespace,
                height: raw_metrics.height,
                ascent,
                descent,
                leading: (first_line.height - ascent - descent).max(0.0),
            };

            let collected = Arc::new(Mutex::new(Vec::new()));
            let renderer: IDWriteTextRenderer = RunCollector {
                runs: Arc::clone(&collected),
                scale,
            }
            .into();
            // SAFETY: `renderer` implements the complete synchronous IDWriteTextRenderer
            // contract; the callback copies every borrowed glyph-run array before returning.
            unsafe { layout.Draw(None, &renderer, origin.x, origin.y) }
                .map_err(directwrite_error)?;
            let runs = collected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let raster_glyphs = collect_raster_glyphs(&self.analysis_factory, &runs, scale)?;
            drop(runs);

            let mut unique = HashSet::new();
            let unique_glyphs = raster_glyphs
                .iter()
                .filter(|glyph| unique.insert(glyph.key.clone()))
                .collect::<Vec<_>>();
            if unique_glyphs.len() > self.atlas.config.max_entries {
                return Err(TextError::AtlasCapacityExceeded {
                    required: unique_glyphs.len(),
                    capacity: self.atlas.config.max_entries,
                });
            }
            if self.atlas.needs_repack(&raster_glyphs) {
                self.atlas.clear_for_repack();
            }
            if !self.atlas.can_pack(unique_glyphs.iter().copied()) {
                let glyph = unique_glyphs.first().map_or(0, |glyph| glyph.glyph);
                return Err(TextError::GlyphTooLarge {
                    glyph,
                    width: unique_glyphs
                        .iter()
                        .map(|glyph| glyph.padded_width)
                        .max()
                        .unwrap_or(0),
                    height: unique_glyphs
                        .iter()
                        .map(|glyph| glyph.padded_height)
                        .max()
                        .unwrap_or(0),
                    atlas_width: self.atlas.config.width,
                    atlas_height: self.atlas.config.height,
                });
            }
            let missing = unique_glyphs
                .iter()
                .copied()
                .filter(|glyph| !self.atlas.entries.contains_key(&glyph.key))
                .map(rasterize)
                .collect::<Result<Vec<_>, _>>()?;
            self.rasterized_glyphs = self
                .rasterized_glyphs
                .saturating_add(u64::try_from(missing.len()).unwrap_or(u64::MAX));
            let atlas_changed = self.atlas.insert(&missing)?;

            let atlas_width = self.atlas.config.width as f32;
            let atlas_height = self.atlas.config.height as f32;
            let inverse_scale = scale.recip();
            let glyphs = raster_glyphs
                .iter()
                .filter_map(|glyph| {
                    let entry = self.atlas.entries.get(&glyph.key)?;
                    Some(Glyph {
                        bounds: Rect {
                            origin: Point {
                                x: (glyph.bounds.left as f32 - GLYPH_PADDING as f32)
                                    * inverse_scale,
                                y: (glyph.bounds.top as f32 - GLYPH_PADDING as f32) * inverse_scale,
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
                fonts: Arc::from(vec![family.to_owned()]),
                atlas_upload: atlas_changed.then(|| self.atlas.upload()),
            })
        }

        fn validate_family(&self, family: &str) -> Result<(), TextError> {
            if family_is_available(&self.font_collection, family)? {
                Ok(())
            } else {
                Err(TextError::FontUnavailable {
                    family: family.to_owned(),
                })
            }
        }
    }

    fn collect_raster_glyphs(
        factory: &IDWriteFactory2,
        runs: &[CollectedRun],
        scale: f32,
    ) -> Result<Vec<RasterGlyph>, TextError> {
        let mut result = Vec::new();
        let transform = DWRITE_MATRIX {
            m11: scale,
            m12: 0.0,
            m21: 0.0,
            m22: scale,
            dx: 0.0,
            dy: 0.0,
        };
        for run in runs {
            let font_identity = font_identity(&run.font_face)?;
            let direction = if run.bidi_level & 1 == 0 { 1.0 } else { -1.0 };
            let mut pen = 0.0_f32;
            for ((glyph, advance), offset) in run
                .glyph_indices
                .iter()
                .copied()
                .zip(run.glyph_advances.iter().copied())
                .zip(run.glyph_offsets.iter().copied())
            {
                let baseline_x = run.baseline_x + pen;
                let baseline_y = run.baseline_y;
                let glyph_index = glyph;
                let glyph_advance = advance;
                let glyph_offset = offset;
                let native_run = DWRITE_GLYPH_RUN {
                    fontFace: ManuallyDrop::new(Some(run.font_face.clone())),
                    fontEmSize: run.font_em_size,
                    glyphCount: 1,
                    glyphIndices: &raw const glyph_index,
                    glyphAdvances: &raw const glyph_advance,
                    glyphOffsets: &raw const glyph_offset,
                    isSideways: run.is_sideways,
                    bidiLevel: run.bidi_level,
                };
                // SAFETY: `native_run` and its single-element arrays remain alive through the
                // call; `transform` scales DIPs into current display pixels and the returned
                // analysis retains the COM font face it needs.
                let analysis = unsafe {
                    factory.CreateGlyphRunAnalysis(
                        &raw const native_run,
                        Some(&raw const transform),
                        rendering_mode_for_size(run.font_em_size, scale),
                        run.measuring_mode,
                        DWRITE_GRID_FIT_MODE_DEFAULT,
                        DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                        baseline_x,
                        baseline_y,
                    )
                }
                .map_err(directwrite_error)?;
                // SAFETY: The analysis object is live and returns a value RECT.
                let bounds = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) }
                    .map_err(directwrite_error)?;
                let width = u32::try_from((bounds.right - bounds.left).max(0)).unwrap_or(u32::MAX);
                let height = u32::try_from((bounds.bottom - bounds.top).max(0)).unwrap_or(u32::MAX);
                if width != 0 && height != 0 {
                    let padded_width =
                        width
                            .checked_add(GLYPH_PADDING * 2)
                            .ok_or(TextError::GlyphTooLarge {
                                glyph,
                                width,
                                height,
                                atlas_width: u32::MAX,
                                atlas_height: u32::MAX,
                            })?;
                    let padded_height =
                        height
                            .checked_add(GLYPH_PADDING * 2)
                            .ok_or(TextError::GlyphTooLarge {
                                glyph,
                                width,
                                height,
                                atlas_width: u32::MAX,
                                atlas_height: u32::MAX,
                            })?;
                    let (subpixel_x, subpixel_y) = subpixel_phase(baseline_x, baseline_y, scale);
                    result.push(RasterGlyph {
                        key: GlyphKey {
                            font_identity,
                            em_size_bits: run.font_em_size.to_bits(),
                            scale_bits: scale.to_bits(),
                            glyph,
                            subpixel_x,
                            subpixel_y,
                        },
                        glyph,
                        analysis,
                        bounds,
                        padded_width,
                        padded_height,
                    });
                }
                pen += advance * direction;
            }
        }
        Ok(result)
    }

    fn copy_grayscale_mask(
        source: &[u8],
        width: u32,
        height: u32,
        padding: u32,
    ) -> Option<Vec<u8>> {
        let source_width = usize::try_from(width).ok()?;
        let source_height = usize::try_from(height).ok()?;
        let expected_source_len = source_width.checked_mul(source_height)?;
        if source.len() != expected_source_len {
            return None;
        }
        let double_padding = padding.checked_mul(2)?;
        let padded_width = width.checked_add(double_padding)?;
        let padded_height = height.checked_add(double_padding)?;
        let target_width = usize::try_from(padded_width).ok()?;
        let target_height = usize::try_from(padded_height).ok()?;
        let padding = usize::try_from(padding).ok()?;
        let mut pixels = vec![0_u8; target_width.checked_mul(target_height)?];
        for (row, source_row) in source.chunks_exact(source_width).enumerate() {
            let start = row
                .checked_add(padding)?
                .checked_mul(target_width)?
                .checked_add(padding)?;
            let end = start.checked_add(source_width)?;
            pixels.get_mut(start..end)?.copy_from_slice(source_row);
        }
        Some(pixels)
    }

    fn rasterize(glyph: &RasterGlyph) -> Result<GlyphBitmap, TextError> {
        let width =
            u32::try_from((glyph.bounds.right - glyph.bounds.left).max(0)).unwrap_or(u32::MAX);
        let height =
            u32::try_from((glyph.bounds.bottom - glyph.bounds.top).max(0)).unwrap_or(u32::MAX);
        let grayscale_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .filter(|length| u32::try_from(*length).is_ok())
            .ok_or(TextError::GlyphTooLarge {
                glyph: glyph.glyph,
                width,
                height,
                atlas_width: glyph.padded_width,
                atlas_height: glyph.padded_height,
            })?;
        let mut grayscale = vec![0_u8; grayscale_len];
        // SAFETY: The buffer is exactly `width * height` bytes for the single-channel texture and
        // `glyph.bounds` is the rectangle obtained from this same grayscale analysis object.
        unsafe {
            glyph.analysis.CreateAlphaTexture(
                DWRITE_TEXTURE_ALIASED_1x1,
                &raw const glyph.bounds,
                &mut grayscale,
            )
        }
        .map_err(directwrite_error)?;
        let pixels = copy_grayscale_mask(&grayscale, width, height, GLYPH_PADDING).ok_or(
            TextError::GlyphTooLarge {
                glyph: glyph.glyph,
                width: glyph.padded_width,
                height: glyph.padded_height,
                atlas_width: glyph.padded_width,
                atlas_height: glyph.padded_height,
            },
        )?;
        Ok(GlyphBitmap {
            key: glyph.key.clone(),
            glyph: glyph.glyph,
            width: glyph.padded_width,
            height: glyph.padded_height,
            pixels,
        })
    }

    fn font_identity(face: &IDWriteFontFace) -> Result<u64, TextError> {
        let mut hasher = DefaultHasher::new();
        // SAFETY: These value-returning methods only inspect the live DirectWrite font face.
        unsafe {
            face.GetType().0.hash(&mut hasher);
            face.GetIndex().hash(&mut hasher);
            face.GetSimulations().0.hash(&mut hasher);
        }
        let mut file_count = 0;
        // SAFETY: Passing no array asks DirectWrite for the required file count.
        unsafe { face.GetFiles(&raw mut file_count, None) }.map_err(directwrite_error)?;
        let mut files: Vec<Option<IDWriteFontFile>> =
            vec![None; usize::try_from(file_count).unwrap_or(usize::MAX)];
        if !files.is_empty() {
            // SAFETY: `files` has exactly the count returned by DirectWrite and remains writable
            // for the duration of the call.
            unsafe { face.GetFiles(&raw mut file_count, Some(files.as_mut_ptr())) }
                .map_err(directwrite_error)?;
        }
        for file in files.iter().flatten() {
            let mut key = std::ptr::null_mut();
            let mut key_size = 0;
            // SAFETY: Both out parameters are valid; DirectWrite owns the returned reference-key
            // bytes for at least as long as the font file COM object retained in this loop.
            unsafe { file.GetReferenceKey(&raw mut key, &raw mut key_size) }
                .map_err(directwrite_error)?;
            if key.is_null() && key_size != 0 {
                return Err(TextError::DirectWrite(
                    "font file returned a null reference key".to_owned(),
                ));
            }
            // SAFETY: The preceding DirectWrite call guarantees `key_size` readable bytes; a null
            // pointer is permitted only for the zero-length case handled by `from_raw_parts`.
            let bytes = unsafe {
                slice::from_raw_parts(
                    key.cast::<u8>(),
                    usize::try_from(key_size).unwrap_or(usize::MAX),
                )
            };
            bytes.hash(&mut hasher);
        }
        Ok(hasher.finish())
    }

    fn subpixel_phase(x: f32, y: f32, scale: f32) -> (u8, u8) {
        const POSITIONS: f32 = 4.0;
        let physical_x = x * scale;
        let physical_y = y * scale;
        let x = ((physical_x - physical_x.floor()) * POSITIONS).floor() as u8;
        let y = ((physical_y - physical_y.floor()) * POSITIONS).floor() as u8;
        (x, y)
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::{
            copy_grayscale_mask, first_language_name, modern_ui_font_size, rendering_mode_for_size,
        };
        use windows::Win32::Graphics::DirectWrite::{
            DWRITE_RENDERING_MODE_NATURAL, DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
        };

        #[test]
        fn first_language_name_reads_only_the_leading_multistring_entry() {
            let languages = "zh-Hans-CN\0en-US\0\0".encode_utf16().collect::<Vec<_>>();

            assert_eq!(first_language_name(&languages).unwrap(), "zh-Hans-CN");
        }

        #[test]
        fn grayscale_mask_copy_preserves_one_coverage_byte_per_pixel_and_padding() {
            let source = [0, 64, 128, 255];

            assert_eq!(
                copy_grayscale_mask(&source, 2, 2, 1).unwrap(),
                vec![
                    0, 0, 0, 0, // top padding
                    0, 0, 64, 0, // first mask row
                    0, 128, 255, 0, // second mask row
                    0, 0, 0, 0, // bottom padding
                ]
            );
        }

        #[test]
        fn modern_ui_font_size_scales_the_legacy_message_font_baseline() {
            assert!((modern_ui_font_size(12.0) - 14.0).abs() < f32::EPSILON);
            assert!((modern_ui_font_size(18.0) - 21.0).abs() < f32::EPSILON);
        }

        #[test]
        fn small_ui_text_uses_natural_rendering_and_large_text_keeps_symmetric_smoothing() {
            assert_eq!(
                rendering_mode_for_size(14.0, 1.0).0,
                DWRITE_RENDERING_MODE_NATURAL.0
            );
            assert_eq!(
                rendering_mode_for_size(18.0, 1.0).0,
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC.0
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub use platform::TextSystem;

#[cfg(not(target_os = "windows"))]
#[derive(Debug)]
pub struct TextSystem;

#[cfg(not(target_os = "windows"))]
impl TextSystem {
    /// Reports that the DirectWrite text system is unavailable on this target.
    ///
    /// # Errors
    ///
    /// Always returns [`TextError::UnsupportedPlatform`].
    pub fn new(_config: AtlasConfig) -> Result<Self, TextError> {
        Err(TextError::UnsupportedPlatform)
    }
}
