#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PointerPhase {
    #[default]
    Idle,
    Moving,
    Pressed,
    Released,
}

#[derive(Clone, Debug, Default)]
pub struct PointerTracker {
    position: (f32, f32),
    pressed_target: Option<u64>,
    phase: PointerPhase,
}

impl PointerTracker {
    pub fn update_position(&mut self, x: f32, y: f32) {
        self.position = (x, y);
        self.phase = PointerPhase::Moving;
    }

    pub fn press(&mut self, target: Option<u64>) {
        self.pressed_target = target;
        self.phase = PointerPhase::Pressed;
    }

    pub fn release(&mut self, target: Option<u64>) -> Option<u64> {
        let clicked = (self.pressed_target == target).then_some(target).flatten();
        self.pressed_target = None;
        self.phase = PointerPhase::Released;
        clicked
    }

    #[must_use]
    pub const fn position(&self) -> (f32, f32) {
        self.position
    }

    #[must_use]
    pub const fn phase(&self) -> PointerPhase {
        self.phase
    }
}

pub struct DisplayCoordinator {
    wake: Box<dyn FnMut()>,
    dirty: bool,
    in_frame: bool,
    submissions: usize,
}

impl DisplayCoordinator {
    #[must_use]
    pub fn new(wake: impl FnMut() + 'static) -> Self {
        Self {
            wake: Box::new(wake),
            dirty: false,
            in_frame: false,
            submissions: 0,
        }
    }

    pub fn invalidate(&mut self) -> bool {
        if self.dirty {
            return false;
        }
        self.dirty = true;
        (self.wake)();
        true
    }

    pub fn begin_frame(&mut self) -> bool {
        if !self.dirty || self.in_frame {
            return false;
        }
        self.dirty = false;
        self.in_frame = true;
        true
    }

    pub fn end_frame(&mut self, submitted: bool) {
        if !self.in_frame {
            return;
        }
        self.in_frame = false;
        if submitted {
            self.submissions += 1;
        }
    }

    #[must_use]
    pub const fn submission_count(&self) -> usize {
        self.submissions
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    logical_width: f32,
    logical_height: f32,
    scale: f32,
    physical_width: u32,
    physical_height: u32,
}

impl Viewport {
    /// Creates a logical viewport and native display scale.
    ///
    /// # Panics
    ///
    /// Panics for negative dimensions or a non-positive/non-finite scale.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn new(logical_width: f32, logical_height: f32, scale: f32) -> Self {
        let physical_width = (logical_width * scale).round() as u32;
        let physical_height = (logical_height * scale).round() as u32;
        Self::with_backing_size(
            logical_width,
            logical_height,
            scale,
            physical_width,
            physical_height,
        )
    }

    /// Creates a viewport with the exact backing-store size reported by the platform.
    ///
    /// # Panics
    ///
    /// Panics for negative logical dimensions or a non-positive/non-finite scale.
    #[must_use]
    pub fn with_backing_size(
        logical_width: f32,
        logical_height: f32,
        scale: f32,
        physical_width: u32,
        physical_height: u32,
    ) -> Self {
        assert!(logical_width >= 0.0 && logical_height >= 0.0);
        assert!(scale.is_finite() && scale > 0.0);
        Self {
            logical_width,
            logical_height,
            scale,
            physical_width,
            physical_height,
        }
    }

    pub fn update(&mut self, logical_width: f32, logical_height: f32, scale: f32) -> bool {
        let next = Self::new(logical_width, logical_height, scale);
        if *self == next {
            return false;
        }
        *self = next;
        true
    }

    pub fn update_backing_size(
        &mut self,
        logical_width: f32,
        logical_height: f32,
        scale: f32,
        physical_width: u32,
        physical_height: u32,
    ) -> bool {
        let next = Self::with_backing_size(
            logical_width,
            logical_height,
            scale,
            physical_width,
            physical_height,
        );
        if *self == next {
            return false;
        }
        *self = next;
        true
    }

    #[must_use]
    pub fn physical_size(&self) -> (u32, u32) {
        (self.physical_width, self.physical_height)
    }

    #[must_use]
    pub const fn logical_size(&self) -> (f32, f32) {
        (self.logical_width, self.logical_height)
    }

    #[must_use]
    pub const fn scale(&self) -> f32 {
        self.scale
    }
}
