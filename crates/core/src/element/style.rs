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
    fn h(self, value: impl Into<Pixels>) -> Self {
        self.height(value)
    }

    #[cfg(feature = "tailwind")]
    #[must_use]
    fn w(self, value: impl Into<Pixels>) -> Self {
        self.width(value)
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
