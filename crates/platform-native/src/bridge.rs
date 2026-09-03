use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    num::NonZeroUsize,
    sync::Arc,
};

use anmixiu_core::{
    AlignItems as CoreAlign, ClickHandler, Color as CoreColor, CursorStyle, ElementId, ElementNode,
    FlexDirection as CoreDirection, GlobalElementId, HoverHandler, InteractiveElement,
    JustifyContent as CoreJustify, ParentElement, Pixels as CorePixels, ScrollHandle, SharedString,
    Style, StyleRefinement, Styled,
};
use anmixiu_layout_taffy::{
    Align, Dimension, Edges, FlexDirection, Justify, LayoutCacheStats, LayoutEngine, LayoutNode,
    LayoutNodeId, LayoutRequest, LayoutRevisions, LayoutStyle, LayoutTree, MeasureId, Viewport,
};
use anmixiu_scene::{
    AtlasUpload, Clip, Color, DrawCommand, Glyph, HitId, HitRegion, Point, Rect, Scene, SceneCache,
    SceneCacheKey, SceneCacheStats, Size,
};
#[cfg(target_os = "macos")]
use anmixiu_text_coretext::{AtlasConfig, FontSpec, ShapedText, TextError, TextSystem};
#[cfg(target_os = "windows")]
use anmixiu_text_directwrite::{AtlasConfig, FontSpec, ShapedText, TextError, TextSystem};
use thiserror::Error;

const FRAME_SCENE_CACHE_CAPACITY: usize = 4;
const SHAPED_TEXT_CACHE_CAPACITY: usize = 512;

#[derive(Clone)]
struct ProjectedNode {
    id: LayoutNodeId,
    parent: Option<LayoutNodeId>,
    global_id: Option<GlobalElementId>,
    style: Style,
    text: Option<SharedString>,
    handler: Option<ClickHandler>,
    hover_handler: Option<HoverHandler>,
    hover_style: Option<StyleRefinement>,
    clickable: bool,
    hoverable: bool,
    is_button: bool,
    scroll: Option<ScrollHandle>,
}

struct Projection {
    root: LayoutNode,
    nodes: Vec<ProjectedNode>,
    fingerprints: Fingerprints,
}

struct FrameInteractions {
    handlers: HashMap<HitId, ClickHandler>,
    element_ids: HashMap<HitId, GlobalElementId>,
    hover_handlers: HashMap<GlobalElementId, HoverHandler>,
    cursor_styles: HashMap<HitId, CursorStyle>,
    click_targets: HashSet<HitId>,
    hover_targets: HashSet<HitId>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Fingerprints {
    structure: u64,
    style: u64,
    measure: u64,
    paint: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HoverTarget {
    Semantic(GlobalElementId),
    Transient(HitId),
}

impl HoverTarget {
    fn matches(&self, id: HitId, global_id: Option<&GlobalElementId>) -> bool {
        match self {
            Self::Semantic(hovered) => global_id == Some(hovered),
            Self::Transient(hovered) => *hovered == id,
        }
    }
}

#[derive(Debug, Error)]
pub enum FrameBuildError {
    #[error(transparent)]
    Layout(#[from] anmixiu_layout_taffy::LayoutError),
    #[error(transparent)]
    Text(#[from] TextError),
    #[error("duplicate semantic element identity `{0}` in one rendered tree")]
    DuplicateElementId(GlobalElementId),
}

/// A scroll container's viewport as painted this frame, with its handle and clamp range, so the
/// platform can route a wheel gesture over the viewport to the right offset.
#[derive(Clone, Debug)]
struct ScrollRegion {
    viewport: Rect,
    handle: ScrollHandle,
    max_offset_x: f32,
    max_offset_y: f32,
}

#[derive(Debug)]
pub struct BuiltFrame {
    pub layout: Arc<LayoutTree>,
    pub scene: Arc<Scene>,
    handlers: HashMap<HitId, ClickHandler>,
    element_ids: HashMap<HitId, GlobalElementId>,
    hover_handlers: HashMap<GlobalElementId, HoverHandler>,
    cursor_styles: HashMap<HitId, CursorStyle>,
    click_targets: HashSet<HitId>,
    hover_targets: HashSet<HitId>,
    scroll_regions: Vec<ScrollRegion>,
}

impl BuiltFrame {
    #[must_use]
    pub fn handler(&self, hit: HitId) -> Option<&ClickHandler> {
        self.handlers.get(&hit)
    }

    /// Returns the frontmost click-capable target at `point`.
    #[must_use]
    pub fn click_target_at(&self, point: Point) -> Option<HitId> {
        self.target_at(point, &self.click_targets)
    }

    /// Returns the frontmost hover-capable target at `point`.
    #[must_use]
    pub fn hover_target_at(&self, point: Point) -> Option<HitId> {
        self.target_at(point, &self.hover_targets)
    }

    fn target_at(&self, point: Point, targets: &HashSet<HitId>) -> Option<HitId> {
        self.scene
            .hit_regions()
            .iter()
            .rev()
            .find(|region| targets.contains(&region.id) && region.contains(point))
            .map(|region| region.id)
    }

    /// Routes a vertical wheel delta to the innermost scroll container under `point`.
    ///
    /// This compatibility helper keeps the original public API; native input uses
    /// [`scroll_at_axes`](Self::scroll_at_axes) so horizontal trackpad gestures are preserved.
    #[must_use]
    pub fn scroll_at(&self, point: anmixiu_scene::Point, delta_y: f32) -> bool {
        let region = self
            .scroll_regions
            .iter()
            .rev()
            .find(|region| region.viewport.contains(point));
        let Some(region) = region else {
            return false;
        };
        if region.max_offset_y <= 0.0 {
            return false;
        }
        let before = region.handle.offset_y();
        let after = region.handle.scroll_by(delta_y, region.max_offset_y);
        (after - before).abs() > f32::EPSILON
    }

    /// Routes a two-dimensional wheel delta to the innermost scroll container under `point`.
    ///
    /// Deltas are in logical pixels, positive X moving content left and positive Y moving content
    /// up. Input is accumulated into a target offset and approached by the display link, so
    /// high-resolution trackpad events remain smooth instead of snapping one event at a time.
    #[must_use]
    pub fn scroll_at_axes(&self, point: anmixiu_scene::Point, delta_x: f32, delta_y: f32) -> bool {
        // Innermost = last painted containing region (children paint after parents).
        let region = self
            .scroll_regions
            .iter()
            .rev()
            .find(|region| region.viewport.contains(point));
        let Some(region) = region else {
            return false;
        };
        if region.max_offset_x <= 0.0 && region.max_offset_y <= 0.0 {
            return false;
        }
        region
            .handle
            .scroll_by_smooth(delta_x, delta_y, region.max_offset_x, region.max_offset_y)
    }

    /// Advances active scroll animations for one display-link interval.
    ///
    /// Returns whether any region still needs another frame. Uses `fold` rather than `any` so every
    /// region is advanced each frame: `advance` steps the animation as a side effect, and `any`
    /// would short-circuit after the first still-animating region, freezing the rest. The
    /// side-effecting call is kept on the left of `||` so it is never skipped once `still` is true.
    #[must_use]
    pub fn advance_scroll(&self, delta_seconds: f32) -> bool {
        // Explicit loop, not `any`/`fold`: `advance` steps the animation as a side effect and must
        // run for every region each frame. `any` would short-circuit after the first still-moving
        // region and freeze the rest.
        let mut still_animating = false;
        for region in &self.scroll_regions {
            if region.handle.advance(delta_seconds) {
                still_animating = true;
            }
        }
        still_animating
    }

    #[must_use]
    pub fn global_id(&self, hit: HitId) -> Option<&GlobalElementId> {
        self.element_ids.get(&hit)
    }

    #[must_use]
    pub fn hover_handler(&self, id: &GlobalElementId) -> Option<&HoverHandler> {
        self.hover_handlers.get(id)
    }

    #[must_use]
    pub fn cursor_style(&self, hit: HitId) -> CursorStyle {
        self.cursor_styles.get(&hit).copied().unwrap_or_default()
    }
}

/// Projects public core elements into internal Taffy and scene contracts.
///
/// Cache keys are complete revision fingerprints plus viewport/scale. Layout holds one current
/// tree, scenes use a four-entry LRU, and shaped text uses a hard 512-entry cache that is cleared
/// as a unit at capacity. Text-measure entries are keyed by value/scale; positioned glyph entries
/// also include the exact final origin so their subpixel phase remains valid. Both text
/// caches clear as a unit at their hard capacity.
pub struct FrameBuilder {
    layout: LayoutEngine,
    scene: SceneCache,
    text: TextSystem,
    font: FontSpec,
    shaped_text: HashMap<TextCacheKey, ShapedText>,
    positioned_text: HashMap<PositionedTextCacheKey, PositionedTextCacheEntry>,
    text_atlas_repacks: u64,
    last: Option<Fingerprints>,
    revisions: LayoutRevisions,
    layout_generation: u64,
    paint_generation: u64,
    hovered: Option<HoverTarget>,
    hover_handler: Option<HoverHandler>,
    focused: Option<GlobalElementId>,
    focus_visible: bool,
    #[cfg(feature = "devtools")]
    inspected: Option<String>,
    #[cfg(feature = "devtools")]
    inspected_node: Option<u64>,
    #[cfg(feature = "devtools")]
    previewed: Option<String>,
    #[cfg(feature = "devtools")]
    previewed_node: Option<u64>,
    last_viewport: Option<(u32, u32, u32)>,
}

impl FrameBuilder {
    /// Creates platform text and bounded frame caches.
    ///
    /// # Errors
    ///
    /// Returns a CoreText/atlas initialization error.
    pub fn new() -> Result<Self, FrameBuildError> {
        Self::new_with_font(FontSpec::system_ui_default())
    }

    /// Creates platform frame caches with a resolved application/window font request.
    ///
    /// # Errors
    ///
    /// Returns a CoreText/atlas initialization error.
    pub fn new_with_font(font: FontSpec) -> Result<Self, FrameBuildError> {
        Ok(Self {
            layout: LayoutEngine::new(),
            scene: SceneCache::new(
                NonZeroUsize::new(FRAME_SCENE_CACHE_CAPACITY).unwrap_or(NonZeroUsize::MIN),
            ),
            text: TextSystem::new(AtlasConfig::default())?,
            font,
            shaped_text: HashMap::with_capacity(SHAPED_TEXT_CACHE_CAPACITY),
            positioned_text: HashMap::with_capacity(SHAPED_TEXT_CACHE_CAPACITY),
            text_atlas_repacks: 0,
            last: None,
            revisions: LayoutRevisions::default(),
            layout_generation: 0,
            paint_generation: 0,
            hovered: None,
            hover_handler: None,
            focused: None,
            focus_visible: false,
            #[cfg(feature = "devtools")]
            inspected: None,
            #[cfg(feature = "devtools")]
            inspected_node: None,
            #[cfg(feature = "devtools")]
            previewed: None,
            #[cfg(feature = "devtools")]
            previewed_node: None,
            last_viewport: None,
        })
    }

    /// Builds or reuses layout/scene data for one logical-pixel viewport.
    ///
    /// # Errors
    ///
    /// Returns structured layout or text errors.
    pub fn build(
        &mut self,
        element: &ElementNode,
        viewport_size: Size,
        scale: f32,
    ) -> Result<BuiltFrame, FrameBuildError> {
        let projection = project(element)?;
        self.advance_revisions(projection.fingerprints, viewport_size, scale);

        let mut measured = HashMap::new();
        let mut latest_upload: Option<AtlasUpload> = None;
        for node in projection.nodes.iter().filter(|node| node.text.is_some()) {
            let Some(text) = node.text.as_ref() else {
                continue;
            };
            let (shape, upload) = self.shape_cached(text, scale)?;
            if let Some(upload) = upload {
                latest_upload = Some(upload);
            }
            measured.insert(MeasureId(node.id.0), shape);
        }

        let layout = self.layout.compute(
            LayoutRequest::new(
                &projection.root,
                Viewport::new(viewport_size, scale),
                self.revisions,
            ),
            |measure, _constraints| {
                measured.get(&measure).map_or(Size::default(), |text| {
                    Size::new(text.metrics.width, text.metrics.height)
                })
            },
        )?;

        let scroll_regions = collect_scroll_regions(&projection.nodes, &layout);
        let scene_key = SceneCacheKey::new(
            projection.root.id().0,
            self.paint_generation,
            self.layout_generation,
            scale,
        );

        // Glyph masks depend on each glyph's final device-space position. Layout must therefore
        // finish before CoreText chooses a horizontal subpixel variant.
        let (shaped, latest_upload) = self.position_text_for_scene(
            &projection,
            &layout,
            &measured,
            scene_key,
            scale,
            latest_upload,
        )?;
        let nodes = &projection.nodes;
        let hovered = self.hovered.clone();
        #[cfg(feature = "devtools")]
        let debug_selection = DebugSelection {
            inspected: self.inspected.as_deref(),
            inspected_node: self.inspected_node,
            previewed: self.previewed.as_deref(),
            previewed_node: self.previewed_node,
        };
        #[cfg(not(feature = "devtools"))]
        let debug_selection = DebugSelection::default();
        let scene = self.scene.get_or_insert_with(scene_key, || {
            build_scene(
                nodes,
                &layout,
                &shaped,
                latest_upload,
                hovered.as_ref(),
                self.focus_visible
                    .then_some(self.focused.as_ref())
                    .flatten(),
                debug_selection,
            )
        });
        let interactions = collect_interactions(projection.nodes);
        Ok(BuiltFrame {
            layout,
            scene,
            handlers: interactions.handlers,
            element_ids: interactions.element_ids,
            hover_handlers: interactions.hover_handlers,
            cursor_styles: interactions.cursor_styles,
            click_targets: interactions.click_targets,
            hover_targets: interactions.hover_targets,
            scroll_regions,
        })
    }

    /// Bumps the paint revision so the next `build` rebuilds the scene against updated scroll
    /// offsets. Scroll state lives in app-owned handles read at scene-build time (not during
    /// component render), so a wheel gesture is not observed by the reactive owner; the platform
    /// calls this to force the repaint instead. Layout is untouched — scrolling never relays out.
    pub fn note_scrolled(&mut self) {
        self.paint_generation = self.paint_generation.saturating_add(1);
    }

    /// Refreshes Windows system-derived text settings and invalidates every result that embeds text
    /// metrics, shaping, fallback, or glyph placement when they changed.
    ///
    /// A language change can affect shaping and fallback for explicit fonts too. Retained atlas
    /// pixels remain safe because their bounded keys include font identity and em size; shaped
    /// text, layout, and scene revisions are invalidated here.
    ///
    /// # Errors
    ///
    /// Returns a structured text error when Windows cannot provide valid UI text settings.
    #[cfg(target_os = "windows")]
    pub fn refresh_system_text_settings(&mut self) -> Result<bool, FrameBuildError> {
        if !self.text.refresh_system_text_settings()? {
            return Ok(false);
        }
        self.shaped_text.clear();
        self.positioned_text.clear();
        self.revisions.measure = self.revisions.measure.saturating_add(1);
        self.layout_generation = self.layout_generation.saturating_add(1);
        self.paint_generation = self.paint_generation.saturating_add(1);
        Ok(true)
    }

    #[must_use]
    pub const fn layout_cache_stats(&self) -> LayoutCacheStats {
        self.layout.stats()
    }

    #[must_use]
    pub const fn scene_cache_stats(&self) -> SceneCacheStats {
        self.scene.stats()
    }

    /// Changes the semantic element receiving hover paint.
    ///
    /// Returns whether a new paint revision was requested. Layout revisions are untouched.
    pub fn set_hovered(&mut self, hovered: Option<GlobalElementId>) -> bool {
        let hovered = hovered.map(HoverTarget::Semantic);
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        self.hover_handler = None;
        self.paint_generation = self.paint_generation.saturating_add(1);
        true
    }

    /// Reconciles hover against the latest frame and invokes enter/leave handlers once.
    ///
    /// Passing `None` means the pointer is outside the native content view. Retaining the active
    /// leave handler here lets a removed or moved element receive its final `false` transition
    /// even when it is no longer present in the newest frame.
    pub fn update_hover(&mut self, frame: &BuiltFrame, point: Option<Point>) -> bool {
        let hit = point.and_then(|point| frame.hover_target_at(point));
        let hovered = hit.map(|hit| {
            frame
                .global_id(hit)
                .cloned()
                .map_or(HoverTarget::Transient(hit), HoverTarget::Semantic)
        });
        let next_handler = hit
            .and_then(|hit| frame.global_id(hit))
            .and_then(|id| frame.hover_handler(id))
            .cloned();
        if self.hovered == hovered {
            self.hover_handler = next_handler;
            return false;
        }

        let previous_handler = self.hover_handler.take();
        self.hovered = hovered;
        self.hover_handler.clone_from(&next_handler);
        self.paint_generation = self.paint_generation.saturating_add(1);
        if let Some(handler) = previous_handler {
            handler.invoke(false);
        }
        if let Some(handler) = next_handler {
            handler.invoke(true);
        }
        true
    }

    /// Changes the semantic element receiving visible keyboard focus.
    pub fn set_focused(&mut self, focused: Option<GlobalElementId>) -> bool {
        let focus_visible = focused.is_some();
        self.set_focus(focused, focus_visible)
    }

    fn set_focus(&mut self, focused: Option<GlobalElementId>, focus_visible: bool) -> bool {
        if self.focused == focused && self.focus_visible == focus_visible {
            return false;
        }
        self.focused = focused;
        self.focus_visible = focus_visible;
        self.paint_generation = self.paint_generation.saturating_add(1);
        true
    }

    /// Focuses the frontmost click target at `point` without showing the keyboard-only focus ring,
    /// or clears focus over empty space.
    pub fn focus_at(&mut self, frame: &BuiltFrame, point: Point) -> bool {
        let focused = frame
            .click_target_at(point)
            .and_then(|hit| frame.global_id(hit))
            .cloned();
        self.set_focus(focused, false)
    }

    #[must_use]
    pub const fn hovered(&self) -> Option<&GlobalElementId> {
        match self.hovered.as_ref() {
            Some(HoverTarget::Semantic(id)) => Some(id),
            Some(HoverTarget::Transient(_)) | None => None,
        }
    }

    #[must_use]
    pub const fn focused(&self) -> Option<&GlobalElementId> {
        self.focused.as_ref()
    }

    #[cfg(feature = "devtools")]
    /// Changes the semantic element highlighted by Dev Tools.
    ///
    /// Returns whether a new paint revision was requested. Layout revisions are untouched.
    pub fn set_inspected(&mut self, inspected: Option<String>) -> bool {
        if self.inspected == inspected && self.inspected_node.is_none() {
            return false;
        }
        self.inspected = inspected;
        self.inspected_node = None;
        self.paint_generation = self.paint_generation.saturating_add(1);
        true
    }

    #[cfg(feature = "devtools")]
    /// Pins the inspected element by dense DFS node index, for nodes without a semantic id.
    ///
    /// Mirrors [`set_previewed_node`](Self::set_previewed_node) so any node — not only those with an
    /// `.id(...)` — can be pinned from the tree, matching the by-index preview path.
    pub fn set_inspected_node(&mut self, inspected_node: Option<u64>) -> bool {
        if self.inspected_node == inspected_node && self.inspected.is_none() {
            return false;
        }
        self.inspected = None;
        self.inspected_node = inspected_node;
        self.paint_generation = self.paint_generation.saturating_add(1);
        true
    }

    #[cfg(feature = "devtools")]
    /// Changes the transient element highlighted by Dev Tools while a tree row is hovered.
    pub fn set_previewed(&mut self, previewed: Option<String>) -> bool {
        if self.previewed == previewed && self.previewed_node.is_none() {
            return false;
        }
        self.previewed = previewed;
        self.previewed_node = None;
        self.paint_generation = self.paint_generation.saturating_add(1);
        true
    }

    #[cfg(feature = "devtools")]
    /// Changes the transient node highlighted by Dev Tools while a tree row is hovered.
    pub fn set_previewed_node(&mut self, previewed_node: Option<u64>) -> bool {
        if self.previewed_node == previewed_node && self.previewed.is_none() {
            return false;
        }
        self.previewed = None;
        self.previewed_node = previewed_node;
        self.paint_generation = self.paint_generation.saturating_add(1);
        true
    }

    #[cfg(feature = "devtools")]
    /// Clears transient Dev Tools hover state while preserving a pinned inspection.
    pub fn clear_preview(&mut self) -> bool {
        if self.previewed.is_none() && self.previewed_node.is_none() {
            return false;
        }
        self.previewed = None;
        self.previewed_node = None;
        self.paint_generation = self.paint_generation.saturating_add(1);
        true
    }

    fn advance_revisions(&mut self, next: Fingerprints, size: Size, scale: f32) {
        if self
            .last
            .is_none_or(|last| last.structure != next.structure)
        {
            self.revisions.structure = self.revisions.structure.saturating_add(1);
        }
        if self.last.is_none_or(|last| last.style != next.style) {
            self.revisions.style = self.revisions.style.saturating_add(1);
        }
        if self.last.is_none_or(|last| last.measure != next.measure) {
            self.revisions.measure = self.revisions.measure.saturating_add(1);
        }
        let viewport = (size.width.to_bits(), size.height.to_bits(), scale.to_bits());
        let layout_changed = self.last.is_none_or(|last| {
            last.structure != next.structure
                || last.style != next.style
                || last.measure != next.measure
        }) || self.last_viewport != Some(viewport);
        if layout_changed {
            self.layout_generation = self.layout_generation.saturating_add(1);
        }
        if layout_changed || self.last.is_none_or(|last| last.paint != next.paint) {
            self.paint_generation = self.paint_generation.saturating_add(1);
        }
        self.last = Some(next);
        self.last_viewport = Some(viewport);
    }

    /// Flushes both text caches if the glyph atlas has repacked since we last observed it. Cached
    /// `ShapedText` bakes absolute atlas UVs; a repack moves every glyph, so entries shaped against
    /// the old layout would sample the wrong pixels. Incremental atlas appends leave existing
    /// glyphs in place and do not advance the repack count, so warm caches survive normal growth.
    fn flush_text_caches_if_atlas_repacked(&mut self) {
        let repacks = self.text.atlas_repacks();
        if repacks != self.text_atlas_repacks {
            self.text_atlas_repacks = repacks;
            self.shaped_text.clear();
            self.positioned_text.clear();
        }
    }

    fn shape_cached(
        &mut self,
        value: &SharedString,
        scale: f32,
    ) -> Result<(ShapedText, Option<AtlasUpload>), TextError> {
        self.flush_text_caches_if_atlas_repacked();
        let key = TextCacheKey {
            value: value.clone(),
            scale_bits: scale.to_bits(),
        };
        if let Some(shape) = self.shaped_text.get(&key) {
            return Ok((shape.clone(), None));
        }
        if self.shaped_text.len() == SHAPED_TEXT_CACHE_CAPACITY {
            self.shaped_text.clear();
        }
        let mut shape =
            self.text
                .shape_scaled(value.as_str(), Point::default(), &self.font, scale)?;
        // Shaping may itself have repacked the atlas, invalidating entries inserted earlier this
        // frame. Flush those (adopting the new epoch) before inserting this fresh entry, which is
        // valid against the current layout.
        self.flush_text_caches_if_atlas_repacked();
        let upload = shape.atlas_upload.take();
        self.shaped_text.insert(key, shape.clone());
        Ok((shape, upload))
    }

    fn shape_positioned_cached(
        &mut self,
        value: &SharedString,
        origin: Point,
        scale: f32,
    ) -> Result<(ShapedText, Option<AtlasUpload>), TextError> {
        self.flush_text_caches_if_atlas_repacked();
        let scale_inverse = scale.recip();
        let anchor = Point::new(
            (origin.x * scale).floor() * scale_inverse,
            (origin.y * scale).floor() * scale_inverse,
        );
        let key = PositionedTextCacheKey {
            value: value.clone(),
            phase_x: subpixel_phase(origin.x * scale),
            phase_y: subpixel_phase(origin.y * scale),
            scale_bits: scale.to_bits(),
        };
        if let Some(entry) = self.positioned_text.get(&key) {
            return Ok((translate_shape(&entry.shape, anchor), None));
        }
        if self.positioned_text.len() == SHAPED_TEXT_CACHE_CAPACITY {
            self.positioned_text.clear();
        }
        let mut shape = self
            .text
            .shape_scaled(value.as_str(), origin, &self.font, scale)?;
        // Shaping may itself have repacked the atlas, invalidating entries inserted earlier this
        // frame. Flush those (adopting the new epoch) before inserting this fresh entry.
        self.flush_text_caches_if_atlas_repacked();
        let upload = shape.atlas_upload.take();
        let relative = translate_shape(&shape, Point::new(-anchor.x, -anchor.y));
        self.positioned_text
            .insert(key, PositionedTextCacheEntry { shape: relative });
        Ok((shape, upload))
    }

    fn position_text_for_scene(
        &mut self,
        projection: &Projection,
        layout: &LayoutTree,
        measured: &HashMap<MeasureId, ShapedText>,
        scene_key: SceneCacheKey,
        scale: f32,
        mut latest_upload: Option<AtlasUpload>,
    ) -> Result<(HashMap<MeasureId, ShapedText>, Option<AtlasUpload>), TextError> {
        let mut shaped = HashMap::new();
        if self.scene.contains(scene_key) {
            return Ok((shaped, latest_upload));
        }
        for node in projection.nodes.iter().filter(|node| node.text.is_some()) {
            let Some(value) = node.text.as_ref() else {
                continue;
            };
            let Some(bounds) = layout.bounds(node.id) else {
                continue;
            };
            let padding = node.style.padding.value();
            let metrics = measured
                .get(&MeasureId(node.id.0))
                .map(|shape| shape.metrics)
                .unwrap_or_default();
            let horizontal_space = (bounds.size.width - padding * 2.0 - metrics.width).max(0.0);
            let vertical_space = (bounds.size.height - padding * 2.0 - metrics.height).max(0.0);
            let origin = Point::new(
                bounds.origin.x
                    + padding
                    + if node.is_button {
                        horizontal_space * 0.5
                    } else {
                        0.0
                    },
                bounds.origin.y
                    + padding
                    + if node.is_button {
                        vertical_space * 0.5
                    } else {
                        0.0
                    },
            );
            let (shape, upload) = self.shape_positioned_cached(value, origin, scale)?;
            if let Some(upload) = upload {
                latest_upload = Some(upload);
            }
            shaped.insert(MeasureId(node.id.0), shape);
        }
        Ok((shaped, latest_upload))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TextCacheKey {
    value: SharedString,
    scale_bits: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PositionedTextCacheKey {
    value: SharedString,
    phase_x: u8,
    phase_y: u8,
    scale_bits: u32,
}

#[derive(Clone, Debug)]
struct PositionedTextCacheEntry {
    shape: ShapedText,
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn subpixel_phase(physical: f32) -> u8 {
    const PHASES: f32 = 16.0;
    ((physical - physical.floor()) * PHASES).floor() as u8
}

fn translate_shape(shape: &ShapedText, delta: Point) -> ShapedText {
    let glyphs = shape
        .glyphs
        .iter()
        .map(|glyph| Glyph {
            bounds: Rect::new(
                Point::new(
                    glyph.bounds.origin.x + delta.x,
                    glyph.bounds.origin.y + delta.y,
                ),
                glyph.bounds.size,
            ),
            uv_bounds: glyph.uv_bounds,
            atlas: glyph.atlas,
        })
        .collect::<Vec<_>>();
    ShapedText {
        metrics: shape.metrics,
        glyphs: Arc::from(glyphs),
        fonts: shape.fonts.clone(),
        atlas_upload: None,
    }
}

fn project(element: &ElementNode) -> Result<Projection, FrameBuildError> {
    let mut nodes = Vec::new();
    let mut next_id = 0_u64;
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let root = project_node(
        element,
        CoreColor::BLACK,
        None,
        &mut next_id,
        &mut nodes,
        &mut path,
        &mut seen,
    )?;
    Ok(Projection {
        root,
        nodes,
        fingerprints: fingerprints(element),
    })
}

fn collect_interactions(nodes: Vec<ProjectedNode>) -> FrameInteractions {
    let mut interactions = FrameInteractions {
        handlers: HashMap::new(),
        element_ids: HashMap::new(),
        hover_handlers: HashMap::new(),
        cursor_styles: HashMap::new(),
        click_targets: HashSet::new(),
        hover_targets: HashSet::new(),
    };
    for node in nodes {
        let hit = HitId(node.id.0);
        if let Some(handler) = node.handler {
            interactions.handlers.insert(hit, handler);
        }
        if let Some(global_id) = node.global_id {
            if let Some(handler) = node.hover_handler {
                interactions
                    .hover_handlers
                    .insert(global_id.clone(), handler);
            }
            interactions.element_ids.insert(hit, global_id);
        }
        if node.clickable {
            interactions.click_targets.insert(hit);
        }
        if node.hoverable {
            interactions.hover_targets.insert(hit);
        }
        if node.clickable || node.hoverable {
            interactions.cursor_styles.insert(hit, node.style.cursor);
        }
    }
    interactions
}

fn project_node(
    element: &ElementNode,
    inherited_foreground: CoreColor,
    parent: Option<LayoutNodeId>,
    next_id: &mut u64,
    nodes: &mut Vec<ProjectedNode>,
    path: &mut Vec<ElementId>,
    seen: &mut HashSet<GlobalElementId>,
) -> Result<LayoutNode, FrameBuildError> {
    let id = LayoutNodeId(*next_id);
    *next_id = next_id.saturating_add(1);
    let has_semantic_id = if let Some(element_id) = element.element_id() {
        path.push(element_id.clone());
        true
    } else {
        false
    };
    let global_id = has_semantic_id.then(|| GlobalElementId::new(path.iter().cloned()));
    if let Some(global_id) = &global_id
        && !seen.insert(global_id.clone())
    {
        return Err(FrameBuildError::DuplicateElementId(global_id.clone()));
    }
    let mut style = element.style_ref().clone();
    let effective_foreground = style.foreground.unwrap_or(inherited_foreground);
    style.foreground = Some(effective_foreground);
    let text = element.text_value().cloned();
    let mut layout_node = LayoutNode::new(id).with_style(layout_style(&style));
    if text.is_some() {
        layout_node = layout_node.with_measure(MeasureId(id.0));
    } else {
        for child in element.children_ref() {
            layout_node = layout_node.with_child(project_node(
                child,
                effective_foreground,
                Some(id),
                next_id,
                nodes,
                path,
                seen,
            )?);
        }
    }
    let handler = element.click_handler().cloned();
    let hover_handler = element.hover_handler().cloned();
    let hover_style = element.hover_style().cloned();
    let clickable = element.kind_name() == "button" || handler.is_some();
    let hoverable = hover_handler.is_some() || hover_style.is_some();
    nodes.push(ProjectedNode {
        id,
        parent,
        global_id,
        style,
        text,
        handler,
        hover_handler,
        hover_style,
        clickable,
        hoverable,
        is_button: element.kind_name() == "button",
        scroll: element.scroll_handle().cloned(),
    });
    if has_semantic_id {
        path.pop();
    }
    Ok(layout_node)
}

fn layout_style(style: &Style) -> LayoutStyle {
    LayoutStyle {
        width: dimension(style.width),
        height: dimension(style.height),
        min_width: dimension(style.min_width),
        min_height: dimension(style.min_height),
        max_width: dimension(style.max_width),
        max_height: dimension(style.max_height),
        padding: Edges::all(style.padding.value()),
        gap: style.gap.value(),
        direction: match style.flex_direction {
            CoreDirection::Row => FlexDirection::Row,
            CoreDirection::Column => FlexDirection::Column,
        },
        align_items: match style.align_items {
            CoreAlign::Start => Align::Start,
            CoreAlign::Center => Align::Center,
            CoreAlign::End => Align::End,
            CoreAlign::Stretch => Align::Stretch,
        },
        align_self: style.align_self.map(|value| match value {
            CoreAlign::Start => Align::Start,
            CoreAlign::Center => Align::Center,
            CoreAlign::End => Align::End,
            CoreAlign::Stretch => Align::Stretch,
        }),
        justify_content: match style.justify_content {
            CoreJustify::Start => Justify::Start,
            CoreJustify::Center => Justify::Center,
            CoreJustify::End => Justify::End,
            CoreJustify::SpaceBetween => Justify::SpaceBetween,
        },
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        flex_basis: Dimension::Auto,
    }
}

fn dimension(value: Option<CorePixels>) -> Dimension {
    value.map_or(Dimension::Auto, |value| Dimension::Points(value.value()))
}

#[derive(Clone, Copy, Default)]
struct DebugSelection<'a> {
    inspected: Option<&'a str>,
    inspected_node: Option<u64>,
    previewed: Option<&'a str>,
    previewed_node: Option<u64>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_scene(
    nodes: &[ProjectedNode],
    layout: &LayoutTree,
    shaped: &HashMap<MeasureId, ShapedText>,
    latest_upload: Option<AtlasUpload>,
    hovered: Option<&HoverTarget>,
    focused: Option<&GlobalElementId>,
    debug_selection: DebugSelection<'_>,
) -> Scene {
    let by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let mut commands = Vec::new();
    let mut hits = Vec::new();
    for id in layout.paint_order() {
        let Some(node) = by_id.get(id).copied() else {
            continue;
        };
        let Some(raw_bounds) = layout.bounds(*id) else {
            continue;
        };
        // Shift this node by the offset of every scroll-container ancestor, and clip it to the
        // nearest scroll viewport. Nodes outside any scroll container are unaffected.
        let (offset_x, offset_y, scroll_clip) = scroll_transform(*id, &by_id, layout);
        let bounds = if offset_x == 0.0 && offset_y == 0.0 {
            raw_bounds
        } else {
            Rect::new(
                Point::new(
                    raw_bounds.origin.x - offset_x,
                    raw_bounds.origin.y - offset_y,
                ),
                raw_bounds.size,
            )
        };
        let hover_style = if hovered
            .is_some_and(|hovered| hovered.matches(HitId(node.id.0), node.global_id.as_ref()))
        {
            node.hover_style.as_ref().map(|refinement| {
                let mut style = node.style.clone();
                refinement.apply_to(&mut style);
                style
            })
        } else {
            None
        };
        let paint_style = hover_style.as_ref().unwrap_or(&node.style);
        // Combine the element's own rounded clip with any scroll-viewport clip.
        let self_clip = Clip::rounded(bounds, paint_style.border_radius.value());
        let clip = scroll_clip.map_or(self_clip, |viewport| {
            Clip::rectangular(self_clip.bounds.intersection(viewport).unwrap_or(viewport))
        });
        if let Some(sigma) = backdrop_sigma(paint_style) {
            commands.push(DrawCommand::BackdropBlur {
                bounds,
                sigma,
                corner_radius: paint_style.border_radius.value().max(0.0),
                clip: scroll_clip.map(Clip::rectangular),
            });
        }
        push_box_commands_clipped(&mut commands, bounds, paint_style, scroll_clip);
        if let Some(shape) = shaped.get(&MeasureId(id.0)) {
            let glyphs = if offset_x == 0.0 && offset_y == 0.0 {
                shape.glyphs.clone()
            } else {
                translate_shape(shape, Point::new(-offset_x, -offset_y)).glyphs
            };
            commands.push(DrawCommand::Glyphs {
                glyphs,
                color: scene_color(paint_style.foreground.unwrap_or(CoreColor::BLACK)),
                clip: Some(clip),
            });
        }
        if node.clickable || node.hoverable {
            hits.push(HitRegion::new(HitId(id.0), bounds, Some(clip)));
        }
        if node.global_id.as_ref() == focused {
            push_focus_outline(&mut commands, bounds, paint_style);
        }
    }
    let preview_color = Color {
        r: 0.15,
        g: 0.75,
        b: 1.0,
        a: 0.95,
    };
    let inspected_color = Color {
        r: 1.0,
        g: 0.45,
        b: 0.1,
        a: 0.95,
    };
    // Resolve the highlight: a transient hover preview wins over a pinned inspection, and each can
    // be addressed either by dense node index (works for any node) or by semantic global id (only
    // nodes with an `.id(...)`). The by-index and by-id inspect paths mirror the preview pair so a
    // tree row without a semantic id can still be pinned.
    let by_index = debug_selection
        .previewed_node
        .map(|index| (index, preview_color))
        .or_else(|| {
            debug_selection
                .inspected_node
                .map(|index| (index, inspected_color))
        });
    let by_id = debug_selection
        .previewed
        .map(|value| (value, preview_color))
        .or_else(|| {
            debug_selection
                .inspected
                .map(|value| (value, inspected_color))
        });
    if let Some((node_id, color)) = by_index
        && let Some(node) = nodes.iter().find(|node| node.id.0 == node_id)
        && let Some(bounds) = layout.bounds(node.id)
    {
        push_debug_outline(&mut commands, bounds, color);
    } else if let Some((highlighted, color)) = by_id
        && let Some(node) = nodes.iter().find(|node| {
            node.global_id
                .as_ref()
                .is_some_and(|global_id| global_id.to_string() == highlighted)
        })
        && let Some(bounds) = layout.bounds(node.id)
    {
        push_debug_outline(&mut commands, bounds, color);
    }
    Scene::new(commands, latest_upload.into_iter().collect(), hits)
}

fn push_debug_outline(commands: &mut Vec<DrawCommand>, bounds: Rect, color: Color) {
    let thickness = 2.0;
    let horizontal = Size::new(bounds.size.width, thickness);
    let vertical = Size::new(thickness, bounds.size.height);
    commands.push(DrawCommand::SolidQuad {
        bounds: Rect::new(bounds.origin, horizontal),
        color,
        clip: None,
    });
    commands.push(DrawCommand::SolidQuad {
        bounds: Rect::new(
            Point::new(bounds.origin.x, bounds.max_y() - thickness),
            horizontal,
        ),
        color,
        clip: None,
    });
    commands.push(DrawCommand::SolidQuad {
        bounds: Rect::new(bounds.origin, vertical),
        color,
        clip: None,
    });
    commands.push(DrawCommand::SolidQuad {
        bounds: Rect::new(
            Point::new(bounds.max_x() - thickness, bounds.origin.y),
            vertical,
        ),
        color,
        clip: None,
    });
}

/// Accumulated scroll offset applied to a node from all its scroll-container ancestors,
/// plus the clip rect of the nearest such ancestor (its unscrolled viewport bounds). A node with no
/// scroll ancestor gets `(0.0, None)` and is painted unchanged.
fn scroll_transform(
    id: LayoutNodeId,
    by_id: &HashMap<LayoutNodeId, &ProjectedNode>,
    layout: &LayoutTree,
) -> (f32, f32, Option<Rect>) {
    let mut offset_x = 0.0;
    let mut offset_y = 0.0;
    let mut nearest_scroll: Option<(LayoutNodeId, f32, f32)> = None;
    // Walk to the root through parents; a scroll container contributes its handle offset, and the
    // nearest one's viewport bounds become the clip.
    let mut current = by_id.get(&id).and_then(|node| node.parent);
    while let Some(parent_id) = current {
        let Some(parent) = by_id.get(&parent_id) else {
            break;
        };
        if let Some(handle) = parent.scroll.as_ref() {
            if nearest_scroll.is_none() {
                nearest_scroll = Some((parent_id, handle.offset_x(), handle.offset_y()));
            }
            offset_x += handle.offset_x();
            offset_y += handle.offset_y();
        }
        current = parent.parent;
    }
    let clip = nearest_scroll
        .as_ref()
        .and_then(|(id, _, _)| layout.bounds(*id))
        .map(|bounds| {
            // A nested viewport moves with its scrolling ancestors, but not with its own handle.
            // `offset_*` contains all ancestor handles, so subtract the nearest one's offset.
            let (clip_offset_x, clip_offset_y) = nearest_scroll
                .map_or((0.0, 0.0), |(_, nearest_x, nearest_y)| {
                    (offset_x - nearest_x, offset_y - nearest_y)
                });
            Rect::new(
                Point::new(
                    bounds.origin.x - clip_offset_x,
                    bounds.origin.y - clip_offset_y,
                ),
                bounds.size,
            )
        });
    (offset_x, offset_y, clip)
}

/// Builds the scroll-container registry for a frame: each scrolled node's painted viewport bounds,
/// its handle, content size, and clamp ranges (`content - viewport`).
fn collect_scroll_regions(nodes: &[ProjectedNode], layout: &LayoutTree) -> Vec<ScrollRegion> {
    let by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    // Build the child adjacency once so each container's content size is a single subtree walk
    // (O(subtree)), rather than testing every node's ancestry per container (previously O(n²·depth)
    // and recomputed every animation frame).
    let mut children: HashMap<LayoutNodeId, Vec<LayoutNodeId>> = HashMap::new();
    for node in nodes {
        if let Some(parent) = node.parent {
            children.entry(parent).or_default().push(node.id);
        }
    }
    let mut regions = Vec::new();
    for node in nodes {
        if let Some(handle) = node.scroll.as_ref()
            && let Some(raw_viewport) = layout.bounds(node.id)
        {
            let (offset_x, offset_y, ancestor_clip) = scroll_transform(node.id, &by_id, layout);
            let shifted_viewport = Rect::new(
                Point::new(
                    raw_viewport.origin.x - offset_x,
                    raw_viewport.origin.y - offset_y,
                ),
                raw_viewport.size,
            );
            let viewport = ancestor_clip.map_or(shifted_viewport, |clip| {
                shifted_viewport
                    .intersection(clip)
                    .unwrap_or(Rect::new(shifted_viewport.origin, Size::default()))
            });
            let content_size = scroll_content_size(node.id, &children, layout);
            // Publish the measured sizes so the application can draw its own scrollbar; the
            // framework no longer renders one. Deduplicated inside the handle.
            handle.set_metrics(
                (raw_viewport.size.width, raw_viewport.size.height),
                (content_size.width, content_size.height),
            );
            regions.push(ScrollRegion {
                viewport,
                handle: handle.clone(),
                max_offset_x: (content_size.width - raw_viewport.size.width).max(0.0),
                max_offset_y: (content_size.height - raw_viewport.size.height).max(0.0),
            });
        }
    }
    regions
}

/// Content size of a scroll container: how far its farthest descendant extends from the viewport's
/// top-left, used to clamp each axis to `content - viewport`. Walks only the container's subtree
/// via the prebuilt `children` adjacency (linear in the subtree size).
fn scroll_content_size(
    container: LayoutNodeId,
    children: &HashMap<LayoutNodeId, Vec<LayoutNodeId>>,
    layout: &LayoutTree,
) -> Size {
    let Some(container_bounds) = layout.bounds(container) else {
        return Size::default();
    };
    let left = container_bounds.origin.x;
    let top = container_bounds.origin.y;
    let mut max_right = container_bounds.origin.x + container_bounds.size.width;
    let mut max_bottom = container_bounds.origin.y;
    // Iterative DFS over the subtree rooted at `container`.
    let mut stack: Vec<LayoutNodeId> = children.get(&container).cloned().unwrap_or_default();
    while let Some(id) = stack.pop() {
        if let Some(bounds) = layout.bounds(id) {
            max_right = max_right.max(bounds.origin.x + bounds.size.width);
            max_bottom = max_bottom.max(bounds.origin.y + bounds.size.height);
        }
        if let Some(grandchildren) = children.get(&id) {
            stack.extend(grandchildren.iter().copied());
        }
    }
    Size::new((max_right - left).max(0.0), (max_bottom - top).max(0.0))
}

fn push_focus_outline(commands: &mut Vec<DrawCommand>, bounds: Rect, style: &Style) {
    let Some(color) = style.focus_ring_color.map(scene_color) else {
        return;
    };
    let width = style.focus_ring_width.value().max(0.0);
    if width <= 0.0 || color.a <= 0.0 {
        return;
    }
    let outer = Rect::new(
        Point::new(bounds.origin.x - width, bounds.origin.y - width),
        Size::new(
            bounds.size.width + width * 2.0,
            bounds.size.height + width * 2.0,
        ),
    );
    commands.push(DrawCommand::RoundedBorder {
        bounds: outer,
        color,
        corner_radius: style.border_radius.value().max(0.0) + width,
        border_width: width,
        clip: None,
    });
}

fn push_box_commands_clipped(
    commands: &mut Vec<DrawCommand>,
    bounds: Rect,
    style: &Style,
    clip: Option<Rect>,
) {
    let border_width = style
        .border_width
        .value()
        .max(0.0)
        .min(bounds.size.width.max(0.0) / 2.0)
        .min(bounds.size.height.max(0.0) / 2.0);
    let border = scene_color(style.border_color);
    if border_width > 0.0 && border.a > 0.0 {
        push_quad(
            commands,
            bounds,
            border,
            style.border_radius.value().max(0.0),
            clip,
        );
    }

    let background = scene_color(style.background);
    if background.a == 0.0 {
        return;
    }
    let background_bounds = if border_width > 0.0 {
        inset_rect(bounds, border_width)
    } else {
        bounds
    };
    push_quad(
        commands,
        background_bounds,
        background,
        (style.border_radius.value() - border_width).max(0.0),
        clip,
    );
}

fn push_quad(
    commands: &mut Vec<DrawCommand>,
    bounds: Rect,
    color: Color,
    radius: f32,
    clip: Option<Rect>,
) {
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return;
    }
    let clip = clip.map(Clip::rectangular);
    if radius > 0.0 {
        commands.push(DrawCommand::RoundedQuad {
            bounds,
            color,
            corner_radius: radius,
            clip,
        });
    } else {
        commands.push(DrawCommand::SolidQuad {
            bounds,
            color,
            clip,
        });
    }
}

fn inset_rect(bounds: Rect, inset: f32) -> Rect {
    Rect::new(
        Point::new(bounds.origin.x + inset, bounds.origin.y + inset),
        Size::new(
            (bounds.size.width - inset * 2.0).max(0.0),
            (bounds.size.height - inset * 2.0).max(0.0),
        ),
    )
}

const fn scene_color(color: CoreColor) -> Color {
    Color {
        r: color.red,
        g: color.green,
        b: color.blue,
        a: color.alpha,
    }
}

fn fingerprints(element: &ElementNode) -> Fingerprints {
    let mut structure = DefaultHasher::new();
    let mut style = DefaultHasher::new();
    let mut measure = DefaultHasher::new();
    let mut paint = DefaultHasher::new();
    hash_element(
        element,
        &mut structure,
        &mut style,
        &mut measure,
        &mut paint,
    );
    Fingerprints {
        structure: structure.finish(),
        style: style.finish(),
        measure: measure.finish(),
        paint: paint.finish(),
    }
}

fn hash_element(
    element: &ElementNode,
    structure: &mut DefaultHasher,
    style: &mut DefaultHasher,
    measure: &mut DefaultHasher,
    paint: &mut DefaultHasher,
) {
    element.kind_name().hash(structure);
    element.element_id().hash(structure);
    element.children_ref().len().hash(structure);
    element.text_content().hash(measure);
    element.text_content().hash(paint);
    hash_style(element.style_ref(), style, paint);
    hash_refinement(element.hover_style(), paint);
    element.click_handler().is_some().hash(paint);
    element.hover_handler().is_some().hash(paint);
    for child in element.children_ref() {
        hash_element(child, structure, style, measure, paint);
    }
}

fn hash_style(style_value: &Style, style: &mut DefaultHasher, paint: &mut DefaultHasher) {
    for value in [
        style_value.width.map(CorePixels::value),
        style_value.height.map(CorePixels::value),
        style_value.min_width.map(CorePixels::value),
        style_value.min_height.map(CorePixels::value),
        style_value.max_width.map(CorePixels::value),
        style_value.max_height.map(CorePixels::value),
    ] {
        value.map(f32::to_bits).hash(style);
    }
    for value in [
        style_value.padding.value(),
        style_value.gap.value(),
        style_value.flex_grow,
        style_value.flex_shrink,
    ] {
        value.to_bits().hash(style);
    }
    (style_value.flex_direction as u8).hash(style);
    (style_value.align_items as u8).hash(style);
    style_value.align_self.map(|value| value as u8).hash(style);
    (style_value.justify_content as u8).hash(style);
    for value in [
        style_value.background.red,
        style_value.background.green,
        style_value.background.blue,
        style_value.background.alpha,
    ] {
        value.to_bits().hash(paint);
    }
    if let Some(sigma) = backdrop_sigma(style_value) {
        0xB10B_u16.hash(paint);
        sigma.to_bits().hash(paint);
    }
    style_value
        .foreground
        .map(|color| {
            (
                color.red.to_bits(),
                color.green.to_bits(),
                color.blue.to_bits(),
                color.alpha.to_bits(),
            )
        })
        .hash(paint);
    style_value.border_radius.value().to_bits().hash(paint);
    style_value.border_width.value().to_bits().hash(paint);
    (style_value.cursor as u8).hash(paint);
    style_value.focus_ring_width.value().to_bits().hash(paint);
    style_value
        .focus_ring_color
        .map(|color| {
            (
                color.red.to_bits(),
                color.green.to_bits(),
                color.blue.to_bits(),
                color.alpha.to_bits(),
            )
        })
        .hash(paint);
    for value in [
        style_value.border_color.red,
        style_value.border_color.green,
        style_value.border_color.blue,
        style_value.border_color.alpha,
    ] {
        value.to_bits().hash(paint);
    }
}

fn backdrop_sigma(style: &Style) -> Option<f32> {
    style
        .backdrop_blur
        .map(CorePixels::value)
        .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
}

fn hash_refinement(value: Option<&StyleRefinement>, paint: &mut DefaultHasher) {
    value.is_some().hash(paint);
    let Some(value) = value else {
        return;
    };
    for color in [value.background, value.foreground, value.border_color] {
        color
            .map(|color| {
                (
                    color.red.to_bits(),
                    color.green.to_bits(),
                    color.blue.to_bits(),
                    color.alpha.to_bits(),
                )
            })
            .hash(paint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positioned_text_cache_reuses_raster_for_integer_translation() {
        let mut builder = FrameBuilder::new().unwrap();
        let value = SharedString::new_static("Resize stable");
        let first = builder
            .shape_positioned_cached(&value, Point::new(20.0, 20.0), 1.0)
            .unwrap();
        let rasterized = builder.text.rasterized_glyph_count();
        let translated = builder
            .shape_positioned_cached(&value, Point::new(21.0, 20.0), 1.0)
            .unwrap();

        assert_eq!(
            builder.text.rasterized_glyph_count(),
            rasterized,
            "integer resize translations must move quads without rerasterizing glyph bitmaps"
        );
        assert_eq!(
            builder.positioned_text.len(),
            1,
            "integer resize translations must reuse one positioned shape cache entry"
        );
        assert!(
            (first.0.glyphs[0].bounds.origin.x - translated.0.glyphs[0].bounds.origin.x).abs()
                > 0.5,
            "the cached glyph geometry still follows the new integer origin"
        );
    }

    #[cfg(feature = "devtools")]
    #[test]
    fn inspect_by_id_and_by_index_are_mutually_exclusive() {
        let mut builder = FrameBuilder::new().unwrap();

        // Pinning by semantic id, then by index, must clear the id (they are one selection).
        assert!(builder.set_inspected(Some("root/button".to_owned())));
        assert_eq!(builder.inspected.as_deref(), Some("root/button"));
        assert!(builder.set_inspected_node(Some(4)));
        assert_eq!(builder.inspected_node, Some(4));
        assert_eq!(
            builder.inspected, None,
            "by-index inspect clears the by-id one"
        );

        // And back the other way.
        assert!(builder.set_inspected(Some("root/panel".to_owned())));
        assert_eq!(builder.inspected.as_deref(), Some("root/panel"));
        assert_eq!(
            builder.inspected_node, None,
            "by-id inspect clears the by-index one"
        );

        // Setting the same by-index value twice is a no-op (no new paint revision requested).
        assert!(builder.set_inspected_node(Some(9)));
        assert!(!builder.set_inspected_node(Some(9)));
    }

    #[test]
    fn atlas_repack_flushes_stale_shaped_text_caches() {
        // A tiny atlas forces a clear + repack once enough distinct glyphs are shaped. After the
        // repack, previously cached shapes hold absolute UVs against the old layout and must be
        // evicted rather than served.
        let mut builder = FrameBuilder::new_with_font(FontSpec::system_ui(16.0)).unwrap();
        // Shrink the atlas so overflow is easy to trigger deterministically.
        builder.text = TextSystem::new(AtlasConfig::new(128, 128, 4)).unwrap();

        let a = SharedString::new_static("AB");
        let (_, first_upload) = builder.shape_cached(&a, 1.0).unwrap();
        assert!(first_upload.is_some(), "first shape uploads the atlas");
        assert_eq!(builder.shaped_text.len(), 1);
        let repacks_before = builder.text.atlas_repacks();

        // Shaping four more distinct glyphs overflows capacity 4 and repacks the atlas.
        let b = SharedString::new_static("CDEF");
        builder.shape_cached(&b, 1.0).unwrap();
        assert_eq!(
            builder.text.atlas_repacks(),
            repacks_before + 1,
            "overflow must repack"
        );
        assert_eq!(
            builder.text_atlas_repacks,
            builder.text.atlas_repacks(),
            "builder adopts the new repack epoch"
        );
        assert_eq!(
            builder.shaped_text.len(),
            1,
            "the repack flushed the stale entry, leaving only the freshly shaped string"
        );

        // The stale "AB" entry must be gone: re-shaping it re-runs against the new layout and
        // re-uploads, rather than returning the cached pre-repack UVs.
        let (_, reshaped_upload) = builder.shape_cached(&a, 1.0).unwrap();
        assert!(
            reshaped_upload.is_some(),
            "re-shaping the evicted string rebuilds it against the current atlas layout"
        );
    }
}
