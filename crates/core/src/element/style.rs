/// A logical-pixel quantity used by public element styles.
///
/// Platform backends convert this value to physical pixels using the target window's current
/// scale; it never denotes a fixed number of framebuffer pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pixels(f32);

impl Pixels {
    /// Returns the logical-pixel quantity.
    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }
}

#[must_use]
/// Creates a logical-pixel quantity.
pub const fn px(value: f32) -> Pixels {
    Pixels(value)
}

impl From<f32> for Pixels {
    fn from(value: f32) -> Self {
        Self(value)
    }
}

impl From<u32> for Pixels {
    #[allow(clippy::cast_precision_loss)]
    fn from(value: u32) -> Self {
        Self(value as f32)
    }
}

/// An sRGB color with normalized, transfer-encoded RGB channels and a linear alpha channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);

    #[must_use]
    pub const fn rgb(red: f32, green: f32, blue: f32) -> Self {
        Self::rgba(red, green, blue, 1.0)
    }

    #[must_use]
    pub const fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// Creates an opaque color from a `0xRRGGBB` integer literal.
    ///
    /// # Panics
    ///
    /// Panics when bits above the low 24 are set. Use [`hex_with_alpha`](Self::hex_with_alpha)
    /// for `0xRRGGBBAA` values.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub const fn hex(value: u32) -> Self {
        assert!(value <= 0x00FF_FFFF, "hex RGB color must use 0xRRGGBB");
        Self::rgb(
            ((value >> 16) & 0xFF) as f32 / 255.0,
            ((value >> 8) & 0xFF) as f32 / 255.0,
            (value & 0xFF) as f32 / 255.0,
        )
    }

    /// Creates a color from a `0xRRGGBBAA` integer literal.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub const fn hex_with_alpha(value: u32) -> Self {
        Self::rgba(
            ((value >> 24) & 0xFF) as f32 / 255.0,
            ((value >> 16) & 0xFF) as f32 / 255.0,
            ((value >> 8) & 0xFF) as f32 / 255.0,
            (value & 0xFF) as f32 / 255.0,
        )
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl From<u32> for Color {
    fn from(value: u32) -> Self {
        Self::hex(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FlexDirection {
    Row,
    #[default]
    Column,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlignItems {
    Start,
    #[default]
    Stretch,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JustifyContent {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorStyle {
    #[default]
    Default,
    Pointer,
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub width: Option<Pixels>,
    pub height: Option<Pixels>,
    pub min_width: Option<Pixels>,
    pub min_height: Option<Pixels>,
    pub max_width: Option<Pixels>,
    pub max_height: Option<Pixels>,
    pub padding: Pixels,
    pub gap: Pixels,
    pub flex_direction: FlexDirection,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub align_items: AlignItems,
    pub align_self: Option<AlignItems>,
    pub justify_content: JustifyContent,
    pub background: Color,
    /// Gaussian backdrop-filter sigma in logical pixels.
    pub backdrop_blur: Option<Pixels>,
    /// Gaussian filter sigma applied to this element and its descendants, in logical pixels.
    pub filter_blur: Option<Pixels>,
    pub foreground: Option<Color>,
    pub border_width: Pixels,
    pub border_color: Color,
    pub border_radius: Pixels,
    pub cursor: CursorStyle,
    pub focus_ring_color: Option<Color>,
    pub focus_ring_width: Pixels,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            padding: px(0.0),
            gap: px(0.0),
            flex_direction: FlexDirection::Column,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            align_items: AlignItems::Stretch,
            align_self: None,
            justify_content: JustifyContent::Start,
            background: Color::TRANSPARENT,
            backdrop_blur: None,
            filter_blur: None,
            foreground: None,
            border_width: px(0.0),
            border_color: Color::TRANSPARENT,
            border_radius: px(0.0),
            cursor: CursorStyle::Default,
            focus_ring_color: None,
            focus_ring_width: px(0.0),
        }
    }
}

/// Styling capability, separate from identity, parenting, and interaction.
pub trait Styled: Sized {
    fn style(&mut self) -> &mut Style;
    fn style_ref(&self) -> &Style;

    #[must_use]
    fn width(mut self, value: impl Into<Pixels>) -> Self {
        self.style().width = Some(value.into());
        self
    }

    #[must_use]
    fn height(mut self, value: impl Into<Pixels>) -> Self {
        self.style().height = Some(value.into());
        self
    }

    #[must_use]
    fn min_width(mut self, value: impl Into<Pixels>) -> Self {
        self.style().min_width = Some(value.into());
        self
    }

    #[must_use]
    fn min_height(mut self, value: impl Into<Pixels>) -> Self {
        self.style().min_height = Some(value.into());
        self
    }

    #[must_use]
    fn max_width(mut self, value: impl Into<Pixels>) -> Self {
        self.style().max_width = Some(value.into());
        self
    }

    #[must_use]
    fn max_height(mut self, value: impl Into<Pixels>) -> Self {
        self.style().max_height = Some(value.into());
        self
    }

    #[must_use]
    fn padding(mut self, value: impl Into<Pixels>) -> Self {
        self.style().padding = value.into();
        self
    }

    #[must_use]
    fn gap(mut self, value: impl Into<Pixels>) -> Self {
        self.style().gap = value.into();
        self
    }

    #[must_use]
    fn flex_row(mut self) -> Self {
        self.style().flex_direction = FlexDirection::Row;
        self
    }

    #[must_use]
    fn flex_column(mut self) -> Self {
        self.style().flex_direction = FlexDirection::Column;
        self
    }

    #[must_use]
    fn flex_grow(mut self, value: f32) -> Self {
        self.style().flex_grow = value;
        self
    }

    #[must_use]
    /// Sets the flex shrink factor.
    fn flex_shrink(mut self, value: f32) -> Self {
        self.style().flex_shrink = value;
        self
    }

    #[must_use]
    fn align(mut self, value: AlignItems) -> Self {
        self.style().align_items = value;
        self
    }

    #[must_use]
    fn align_self(mut self, value: AlignItems) -> Self {
        self.style().align_self = Some(value);
        self
    }

    #[must_use]
    fn justify(mut self, value: JustifyContent) -> Self {
        self.style().justify_content = value;
        self
    }

    #[must_use]
    fn background(mut self, value: impl Into<Color>) -> Self {
        self.style().background = value.into();
        self
    }

    /// Applies a Gaussian blur to content painted behind this element.
    ///
    /// The sigma is measured in logical pixels. Non-positive and non-finite values are retained in
    /// the style but produce no scene effect; platform renderers clamp larger finite values to 64
    /// logical pixels.
    #[must_use]
    fn backdrop_blur(mut self, sigma: impl Into<Pixels>) -> Self {
        self.style().backdrop_blur = Some(sigma.into());
        self
    }

    /// Applies a Gaussian filter to this element's painted content and descendants.
    ///
    /// Unlike [`backdrop_blur`](Self::backdrop_blur), this never samples content behind the
    /// element. The sigma is measured in logical pixels. Non-positive and non-finite values are
    /// retained in the style but produce no scene effect; platform renderers clamp larger finite
    /// values to 64 logical pixels.
    #[must_use]
    fn filter_blur(mut self, sigma: impl Into<Pixels>) -> Self {
        self.style().filter_blur = Some(sigma.into());
        self
    }

    #[must_use]
    fn foreground(mut self, value: impl Into<Color>) -> Self {
        self.style().foreground = Some(value.into());
        self
    }

    #[must_use]
    fn rounded(mut self, value: impl Into<Pixels>) -> Self {
        self.style().border_radius = value.into();
        self
    }

    #[must_use]
    fn border_width(mut self, value: impl Into<Pixels>) -> Self {
        self.style().border_width = value.into();
        self
    }

    #[must_use]
    fn border_color(mut self, value: impl Into<Color>) -> Self {
        self.style().border_color = value.into();
        self
    }

    #[must_use]
    fn cursor(mut self, value: CursorStyle) -> Self {
        self.style().cursor = value;
        self
    }

    #[must_use]
    fn focus_ring(mut self, color: impl Into<Color>, width: impl Into<Pixels>) -> Self {
        self.style().focus_ring_color = Some(color.into());
        self.style().focus_ring_width = width.into();
        self
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`width`](Self::width).
    fn w(self, value: impl Into<Pixels>) -> Self {
        self.width(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`height`](Self::height).
    fn h(self, value: impl Into<Pixels>) -> Self {
        self.height(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`min_width`](Self::min_width).
    fn min_w(self, value: impl Into<Pixels>) -> Self {
        self.min_width(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`min_height`](Self::min_height).
    fn min_h(self, value: impl Into<Pixels>) -> Self {
        self.min_height(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`max_width`](Self::max_width).
    fn max_w(self, value: impl Into<Pixels>) -> Self {
        self.max_width(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`max_height`](Self::max_height).
    fn max_h(self, value: impl Into<Pixels>) -> Self {
        self.max_height(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`padding`](Self::padding).
    fn p(self, value: impl Into<Pixels>) -> Self {
        self.padding(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`flex_column`](Self::flex_column).
    fn flex_col(self) -> Self {
        self.flex_column()
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`flex_grow`](Self::flex_grow).
    fn grow(self, value: f32) -> Self {
        self.flex_grow(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`flex_shrink`](Self::flex_shrink).
    fn shrink(self, value: f32) -> Self {
        self.flex_shrink(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`align`](Self::align).
    fn items(self, value: AlignItems) -> Self {
        self.align(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Aligns children to the start of the cross axis.
    fn items_start(self) -> Self {
        self.align(AlignItems::Start)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Stretches children across the cross axis.
    fn items_stretch(self) -> Self {
        self.align(AlignItems::Stretch)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Centers children on the cross axis.
    fn items_center(self) -> Self {
        self.align(AlignItems::Center)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Aligns children to the end of the cross axis.
    fn items_end(self) -> Self {
        self.align(AlignItems::End)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Aligns this flex item to the start of the cross axis.
    fn self_start(self) -> Self {
        self.align_self(AlignItems::Start)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Stretches this flex item across the cross axis.
    fn self_stretch(self) -> Self {
        self.align_self(AlignItems::Stretch)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Centers this flex item on the cross axis.
    fn self_center(self) -> Self {
        self.align_self(AlignItems::Center)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Aligns this flex item to the end of the cross axis.
    fn self_end(self) -> Self {
        self.align_self(AlignItems::End)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Packs children at the start of the main axis.
    fn justify_start(self) -> Self {
        self.justify(JustifyContent::Start)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Centers children on the main axis.
    fn justify_center(self) -> Self {
        self.justify(JustifyContent::Center)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Packs children at the end of the main axis.
    fn justify_end(self) -> Self {
        self.justify(JustifyContent::End)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Distributes children with equal space between adjacent items.
    fn justify_between(self) -> Self {
        self.justify(JustifyContent::SpaceBetween)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`background`](Self::background).
    fn bg(self, value: impl Into<Color>) -> Self {
        self.background(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`filter_blur`](Self::filter_blur).
    fn blur(self, sigma: impl Into<Pixels>) -> Self {
        self.filter_blur(sigma)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`foreground`](Self::foreground).
    fn text_color(self, value: impl Into<Color>) -> Self {
        self.foreground(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`border_width`](Self::border_width).
    fn border(self, value: impl Into<Pixels>) -> Self {
        self.border_width(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`focus_ring`](Self::focus_ring).
    fn ring(self, color: impl Into<Color>, width: impl Into<Pixels>) -> Self {
        self.focus_ring(color, width)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Uses the platform's default cursor.
    fn cursor_default(self) -> Self {
        self.cursor(CursorStyle::Default)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Uses a pointer cursor.
    fn cursor_pointer(self) -> Self {
        self.cursor(CursorStyle::Pointer)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Uses a text-selection cursor.
    fn cursor_text(self) -> Self {
        self.cursor(CursorStyle::Text)
    }
}

/// Paint-only style overrides used by transient interaction states.
///
/// Layout-affecting fields are intentionally absent so hover never invalidates Taffy layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleRefinement {
    pub background: Option<Color>,
    pub foreground: Option<Color>,
    pub border_color: Option<Color>,
}

impl StyleRefinement {
    #[must_use]
    pub fn background(mut self, value: impl Into<Color>) -> Self {
        self.background = Some(value.into());
        self
    }

    #[must_use]
    pub fn foreground(mut self, value: impl Into<Color>) -> Self {
        self.foreground = Some(value.into());
        self
    }

    #[must_use]
    pub fn border_color(mut self, value: impl Into<Color>) -> Self {
        self.border_color = Some(value.into());
        self
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`background`](Self::background).
    pub fn bg(self, value: impl Into<Color>) -> Self {
        self.background(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    /// Tailwind-style alias for [`foreground`](Self::foreground).
    pub fn text_color(self, value: impl Into<Color>) -> Self {
        self.foreground(value)
    }

    #[doc(hidden)]
    pub fn apply_to(&self, style: &mut Style) {
        if let Some(background) = self.background {
            style.background = background;
        }
        if let Some(foreground) = self.foreground {
            style.foreground = Some(foreground);
        }
        if let Some(border_color) = self.border_color {
            style.border_color = border_color;
        }
    }
}
