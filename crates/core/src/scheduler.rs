use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WindowId(u64);

impl WindowId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameLoopError {
    pub window: WindowId,
    pub consecutive_invalidations: usize,
}

#[derive(Default)]
struct WindowFrame {
    dirty: BTreeSet<u64>,
    deferred: BTreeSet<u64>,
    parents: HashMap<u64, u64>,
    requested: bool,
    in_frame: bool,
    requests: usize,
    submissions: usize,
    invalidation_streak: usize,
    loop_error: Option<FrameLoopError>,
}

pub struct FrameBatcher {
    max_consecutive_invalidations: usize,
    windows: HashMap<WindowId, WindowFrame>,
}

impl FrameBatcher {
    /// Creates a per-window batching model with a consecutive-invalidation guard.
    ///
    /// # Panics
    ///
    /// Panics when `max_consecutive_invalidations` is zero.
    #[must_use]
    pub fn new(max_consecutive_invalidations: usize) -> Self {
        assert!(max_consecutive_invalidations > 0);
        Self {
            max_consecutive_invalidations,
            windows: HashMap::new(),
        }
    }

    pub fn mark_dirty(&mut self, window: WindowId, component: u64, parent: Option<u64>) {
        let frame = self.windows.entry(window).or_default();
        if let Some(parent) = parent {
            frame.parents.insert(component, parent);
        }
        if frame.in_frame {
            frame.deferred.insert(component);
            return;
        }
        frame.dirty.insert(component);
        if !frame.requested {
            frame.requested = true;
            frame.requests += 1;
        }
    }

    /// Drops all batching state for a window that has closed. Without this a long-lived process
    /// that opens and closes windows would retain every closed window's dirty sets and parent map
    /// forever.
    pub fn forget_window(&mut self, window: WindowId) {
        self.windows.remove(&window);
    }

    /// Drops a single component's tracking after it unmounts, evicting its parent edge and any
    /// pending dirty/deferred marks. The `parents` map is otherwise append-only, so an app that
    /// mounts and unmounts many components over its lifetime would leak an entry per dead id.
    pub fn forget_component(&mut self, window: WindowId, component: u64) {
        if let Some(frame) = self.windows.get_mut(&window) {
            frame.parents.remove(&component);
            frame.dirty.remove(&component);
            frame.deferred.remove(&component);
        }
    }

    #[must_use]
    pub fn begin_frame(&mut self, window: WindowId) -> Vec<u64> {
        let frame = self.windows.entry(window).or_default();
        if frame.dirty.is_empty() {
            frame.requested = false;
            return Vec::new();
        }
        frame.in_frame = true;
        frame.requested = false;
        let dirty = std::mem::take(&mut frame.dirty);
        dirty
            .iter()
            .copied()
            .filter(|component| !has_dirty_ancestor(*component, &dirty, &frame.parents))
            .collect()
    }

    #[must_use]
    pub fn finish_frame(&mut self, window: WindowId, submitted: bool) -> bool {
        let frame = self.windows.entry(window).or_default();
        frame.in_frame = false;
        if submitted {
            frame.submissions += 1;
        }
        if frame.deferred.is_empty() {
            frame.invalidation_streak = 0;
            return false;
        }
        frame.invalidation_streak += 1;
        if frame.invalidation_streak >= self.max_consecutive_invalidations {
            frame.loop_error = Some(FrameLoopError {
                window,
                consecutive_invalidations: frame.invalidation_streak,
            });
            frame.deferred.clear();
            return false;
        }
        frame.dirty = std::mem::take(&mut frame.deferred);
        frame.requested = true;
        frame.requests += 1;
        true
    }

    #[must_use]
    pub fn frame_requests(&self, window: WindowId) -> usize {
        self.windows.get(&window).map_or(0, |frame| frame.requests)
    }

    #[must_use]
    pub fn submissions(&self, window: WindowId) -> usize {
        self.windows
            .get(&window)
            .map_or(0, |frame| frame.submissions)
    }

    pub fn take_loop_error(&mut self, window: WindowId) -> Option<FrameLoopError> {
        self.windows
            .get_mut(&window)
            .and_then(|frame| frame.loop_error.take())
    }
}

fn has_dirty_ancestor(component: u64, dirty: &BTreeSet<u64>, parents: &HashMap<u64, u64>) -> bool {
    let mut current = component;
    let mut remaining = parents.len() + 1;
    while let Some(parent) = parents.get(&current).copied() {
        if dirty.contains(&parent) {
            return true;
        }
        if remaining == 0 {
            break;
        }
        remaining -= 1;
        current = parent;
    }
    false
}
