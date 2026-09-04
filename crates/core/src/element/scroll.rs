use std::{
    cell::Cell,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use anmixiu_reactive::Signal;

static NEXT_SCROLL_ID: AtomicU64 = AtomicU64::new(1);

/// Exponential approach rate used by platform scroll animations.
///
/// At 120 Hz it moves roughly 15% of the remaining distance per frame, while at 60 Hz it moves
/// roughly 28%, which keeps trackpad input responsive without snapping between wheel events.
const SCROLL_SMOOTHING_RATE: f32 = 20.0;

/// A shared, app-owned two-dimensional scroll offset for one scroll container.
///
/// The application creates a `ScrollHandle`, keeps it in component state, and passes it to
/// [`DivElement::scroll`](super::DivElement::scroll). Wheel events on the container update the
/// handle (clamped to the scrollable range), and the platform repaints the next display frame.
/// The framework never stores scroll state itself — the handle is the single source of truth,
/// consistent with the rest of the signal model.
#[derive(Clone, Debug)]
pub struct ScrollHandle {
    inner: Rc<ScrollState>,
}

#[derive(Debug)]
struct ScrollState {
    id: u64,
    paint_revision: Cell<u64>,
    paint_invalidation: Signal<u64>,
    offset_x: Signal<f32>,
    offset_y: Signal<f32>,
    target_x: Signal<f32>,
    target_y: Signal<f32>,
    // Measured by the framework each layout and read back by the app to draw its own scrollbar.
    // `(width, height)` pairs so a single dedup'd write updates both axes together.
    viewport_size: Signal<(f32, f32)>,
    content_size: Signal<(f32, f32)>,
}

impl Default for ScrollHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollHandle {
    /// Creates a handle scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Rc::new(ScrollState {
                id: NEXT_SCROLL_ID.fetch_add(1, Ordering::Relaxed),
                paint_revision: Cell::new(0),
                paint_invalidation: Signal::new(0),
                offset_x: Signal::new(0.0),
                offset_y: Signal::new(0.0),
                target_x: Signal::new(0.0),
                target_y: Signal::new(0.0),
                viewport_size: Signal::new((0.0, 0.0)),
                content_size: Signal::new((0.0, 0.0)),
            }),
        }
    }

    /// Current horizontal offset in logical pixels (content is shifted left by this much).
    #[must_use]
    pub fn offset_x(&self) -> f32 {
        self.inner.offset_x.get()
    }

    /// Current vertical offset in logical pixels (content is shifted up by this much).
    #[must_use]
    pub fn offset_y(&self) -> f32 {
        self.inner.offset_y.get()
    }

    /// Current target horizontal offset used by smooth scrolling.
    #[must_use]
    pub fn target_x(&self) -> f32 {
        self.inner.target_x.get()
    }

    /// Current target vertical offset used by smooth scrolling.
    #[must_use]
    pub fn target_y(&self) -> f32 {
        self.inner.target_y.get()
    }

    /// Sets the horizontal offset directly (clamped to be non-negative).
    pub fn set_offset_x(&self, value: f32) {
        let value = finite(value).max(0.0);
        let changed = (self.inner.offset_x.get() - value).abs() > f32::EPSILON;
        self.inner.offset_x.set(value);
        self.inner.target_x.set(value);
        if changed {
            self.bump_paint(true);
        }
    }

    /// Sets the vertical offset directly (clamped to be non-negative). Marks subscribers dirty only
    /// when the value actually changes.
    pub fn set_offset_y(&self, value: f32) {
        let value = finite(value).max(0.0);
        let changed = (self.inner.offset_y.get() - value).abs() > f32::EPSILON;
        self.inner.offset_y.set(value);
        self.inner.target_y.set(value);
        if changed {
            self.bump_paint(true);
        }
    }

    /// Applies a wheel delta against a known scrollable range, clamping to `0..=max_offset`.
    /// Returns the resulting offset. `max_offset` is `content_height - viewport_height`.
    #[must_use]
    pub fn scroll_by(&self, delta_y: f32, max_offset: f32) -> f32 {
        self.scroll_by_y(delta_y, max_offset, true)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn scroll_by_native(&self, delta_y: f32, max_offset: f32) -> f32 {
        self.scroll_by_y(delta_y, max_offset, false)
    }

    fn scroll_by_y(&self, delta_y: f32, max_offset: f32, notify: bool) -> f32 {
        let max = finite(max_offset).max(0.0);
        let current = self.inner.offset_y.get();
        let next = (current + finite(delta_y)).clamp(0.0, max);
        self.inner.offset_y.set(next);
        self.inner.target_y.set(next);
        if (next - current).abs() > f32::EPSILON {
            self.bump_paint(notify);
        }
        next
    }

    /// Applies a horizontal wheel delta immediately, clamping to `0..=max_offset`.
    #[must_use]
    pub fn scroll_by_x(&self, delta_x: f32, max_offset: f32) -> f32 {
        let max = finite(max_offset).max(0.0);
        let current = self.inner.offset_x.get();
        let next = (current + finite(delta_x)).clamp(0.0, max);
        self.inner.offset_x.set(next);
        self.inner.target_x.set(next);
        if (next - current).abs() > f32::EPSILON {
            self.bump_paint(true);
        }
        next
    }

    /// Adds wheel deltas to the smooth-scrolling targets.
    ///
    /// The current offsets stay where they are until [`advance`](Self::advance) is called by the
    /// display link. This makes a burst of trackpad events coalesce into one frame-paced motion.
    /// Returns whether either target changed.
    #[must_use]
    pub fn scroll_by_smooth(
        &self,
        delta_x: f32,
        delta_y: f32,
        max_offset_x: f32,
        max_offset_y: f32,
    ) -> bool {
        self.scroll_by_smooth_inner(delta_x, delta_y, max_offset_x, max_offset_y, true)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn scroll_by_smooth_native(
        &self,
        delta_x: f32,
        delta_y: f32,
        max_offset_x: f32,
        max_offset_y: f32,
    ) -> bool {
        self.scroll_by_smooth_inner(delta_x, delta_y, max_offset_x, max_offset_y, false)
    }

    fn scroll_by_smooth_inner(
        &self,
        delta_x: f32,
        delta_y: f32,
        max_offset_x: f32,
        max_offset_y: f32,
        notify: bool,
    ) -> bool {
        let max_x = finite(max_offset_x).max(0.0);
        let max_y = finite(max_offset_y).max(0.0);
        let target_x = self.inner.target_x.get();
        let target_y = self.inner.target_y.get();
        let next_x = (target_x + finite(delta_x)).clamp(0.0, max_x);
        let next_y = (target_y + finite(delta_y)).clamp(0.0, max_y);
        let changed =
            (next_x - target_x).abs() > f32::EPSILON || (next_y - target_y).abs() > f32::EPSILON;
        self.inner.target_x.set(next_x);
        self.inner.target_y.set(next_y);
        if changed && notify {
            self.notify_paint();
        }
        changed
    }

    /// Advances both axes toward their targets for one display-link interval.
    ///
    /// Returns whether another animation frame is needed. Non-finite or non-positive durations are
    /// treated as a single immediate step, which keeps callers from getting stuck on bad timing
    /// data.
    #[must_use]
    pub fn advance(&self, delta_seconds: f32) -> bool {
        self.advance_inner(delta_seconds, true)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn advance_native(&self, delta_seconds: f32) -> bool {
        self.advance_inner(delta_seconds, false)
    }

    fn advance_inner(&self, delta_seconds: f32, notify: bool) -> bool {
        let current_x = self.inner.offset_x.get();
        let current_y = self.inner.offset_y.get();
        let target_x = self.inner.target_x.get();
        let target_y = self.inner.target_y.get();
        let alpha = if delta_seconds.is_finite() && delta_seconds > 0.0 {
            1.0 - (-SCROLL_SMOOTHING_RATE * delta_seconds).exp()
        } else {
            1.0
        };
        let next_x = if (target_x - current_x).abs() < 0.01 {
            target_x
        } else {
            current_x + (target_x - current_x) * alpha
        };
        let next_y = if (target_y - current_y).abs() < 0.01 {
            target_y
        } else {
            current_y + (target_y - current_y) * alpha
        };
        let changed =
            (next_x - current_x).abs() > f32::EPSILON || (next_y - current_y).abs() > f32::EPSILON;
        self.inner.offset_x.set(next_x);
        self.inner.offset_y.set(next_y);
        if changed {
            self.bump_paint(notify);
        }
        changed || (target_x - next_x).abs() >= 0.01 || (target_y - next_y).abs() >= 0.01
    }

    /// Returns whether either axis is still moving toward its smooth-scroll target.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        (self.inner.target_x.get() - self.inner.offset_x.get()).abs() >= 0.01
            || (self.inner.target_y.get() - self.inner.offset_y.get()).abs() >= 0.01
    }

    /// Records the measured viewport and content sizes for this scroll container.
    ///
    /// Called by the platform after layout. The values are deduplicated, so a stable layout does
    /// not churn subscribers. Applications read them back via [`viewport_size`](Self::viewport_size)
    /// / [`content_size`](Self::content_size) to draw their own scrollbar — the framework does not
    /// render one.
    pub fn set_metrics(&self, viewport: (f32, f32), content: (f32, f32)) {
        self.inner
            .viewport_size
            .set((finite(viewport.0).max(0.0), finite(viewport.1).max(0.0)));
        self.inner
            .content_size
            .set((finite(content.0).max(0.0), finite(content.1).max(0.0)));
    }

    /// Measured viewport size `(width, height)` of the scroll container, or `(0, 0)` before the
    /// first layout.
    #[must_use]
    pub fn viewport_size(&self) -> (f32, f32) {
        self.inner.viewport_size.get()
    }

    /// Measured content size `(width, height)` — the full extent of the scrolled content.
    #[must_use]
    pub fn content_size(&self) -> (f32, f32) {
        self.inner.content_size.get()
    }

    /// Maximum vertical offset (`content_height - viewport_height`, floored at 0).
    #[must_use]
    pub fn max_offset_y(&self) -> f32 {
        (self.inner.content_size.get().1 - self.inner.viewport_size.get().1).max(0.0)
    }

    /// Maximum horizontal offset (`content_width - viewport_width`, floored at 0).
    #[must_use]
    pub fn max_offset_x(&self) -> f32 {
        (self.inner.content_size.get().0 - self.inner.viewport_size.get().0).max(0.0)
    }

    pub(crate) fn track_paint(&self) {
        let _revision = self.inner.paint_invalidation.get();
    }

    #[doc(hidden)]
    #[must_use]
    pub fn paint_key(&self) -> (u64, u64) {
        (self.inner.id, self.inner.paint_revision.get())
    }

    fn bump_paint(&self, notify: bool) {
        let revision = self.inner.paint_revision.get().wrapping_add(1);
        self.inner.paint_revision.set(revision);
        if notify {
            self.notify_paint();
        }
    }

    /// Wakes render observers subscribed through [`track_paint`](Self::track_paint).
    ///
    /// `paint_invalidation` is a standalone monotonic tick, not a mirror of `paint_revision`: its
    /// numeric value is never read for meaning, only its *change* matters. Keeping it independent
    /// means a smooth-scroll wake followed by the `advance` that first moves the offset are two
    /// distinct values, so the second notification is not swallowed by `Signal::set`'s equal-value
    /// skip.
    fn notify_paint(&self) {
        self.inner
            .paint_invalidation
            .set(self.inner.paint_invalidation.get().wrapping_add(1));
    }
}

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::ScrollHandle;

    #[test]
    fn scroll_by_clamps_to_the_scrollable_range() {
        let handle = ScrollHandle::new();
        assert!((handle.offset_y() - 0.0).abs() < f32::EPSILON);

        // Scroll down within range.
        assert!((handle.scroll_by(30.0, 100.0) - 30.0).abs() < f32::EPSILON);
        assert!((handle.scroll_by(50.0, 100.0) - 80.0).abs() < f32::EPSILON);

        // Past the end clamps to max_offset.
        assert!((handle.scroll_by(50.0, 100.0) - 100.0).abs() < f32::EPSILON);

        // Scrolling back up clamps at zero.
        assert!((handle.scroll_by(-250.0, 100.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn non_scrollable_content_stays_at_zero() {
        let handle = ScrollHandle::new();
        // max_offset 0 (content fits) means any delta is pinned to 0.
        assert!((handle.scroll_by(40.0, 0.0) - 0.0).abs() < f32::EPSILON);
        assert!((handle.offset_y() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_offset_is_floored_at_zero() {
        let handle = ScrollHandle::new();
        handle.set_offset_y(-5.0);
        assert!((handle.offset_y() - 0.0).abs() < f32::EPSILON);
        handle.set_offset_y(42.0);
        assert!((handle.offset_y() - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn smooth_scroll_coalesces_targets_and_converges() {
        let handle = ScrollHandle::new();
        assert!(handle.scroll_by_smooth(120.0, 80.0, 300.0, 200.0));
        assert!((handle.offset_x() - 0.0).abs() < f32::EPSILON);
        assert!((handle.offset_y() - 0.0).abs() < f32::EPSILON);
        assert!((handle.target_x() - 120.0).abs() < f32::EPSILON);
        assert!((handle.target_y() - 80.0).abs() < f32::EPSILON);
        assert!(handle.advance(1.0 / 60.0));
        assert!(handle.offset_x() > 0.0 && handle.offset_x() < 120.0);
        for _ in 0..120 {
            let _ = handle.advance(1.0 / 60.0);
        }
        assert!((handle.offset_x() - 120.0).abs() < 0.01);
        assert!((handle.offset_y() - 80.0).abs() < 0.01);
        assert!(!handle.is_animating());
    }

    #[test]
    fn smooth_scroll_then_advance_dirties_the_owner_on_the_frame_the_offset_moves() {
        // A render observer that reads paint invalidation must be re-dirtied both when a smooth
        // scroll requests animation and when `advance` actually moves the offset. If the two
        // share a paint token value, the second write is suppressed and the frame that first
        // shows motion is never repainted.
        use anmixiu_reactive::OwnerRegistry;

        let registry = OwnerRegistry::new();
        let owner = registry.create_owner();
        let handle = ScrollHandle::new();
        let render = || {
            registry.observe(owner, || handle.track_paint());
        };

        render();
        let _ = registry.take_dirty();

        assert!(handle.scroll_by_smooth(0.0, 80.0, 0.0, 200.0));
        assert_eq!(
            registry.take_dirty(),
            vec![owner],
            "queuing a smooth-scroll target must wake the render observer"
        );

        render();
        assert!(
            handle.advance(1.0 / 60.0),
            "the offset moves toward the target"
        );
        assert_eq!(
            registry.take_dirty(),
            vec![owner],
            "the frame that first moves the offset must re-dirty the observer"
        );
    }

    #[test]
    fn direct_set_keeps_smooth_target_in_sync() {
        let handle = ScrollHandle::new();
        let _ = handle.scroll_by_smooth(40.0, 50.0, 100.0, 100.0);
        handle.set_offset_x(25.0);
        handle.set_offset_y(30.0);
        assert!((handle.target_x() - 25.0).abs() < f32::EPSILON);
        assert!((handle.target_y() - 30.0).abs() < f32::EPSILON);
        assert!(!handle.is_animating());
    }
}
