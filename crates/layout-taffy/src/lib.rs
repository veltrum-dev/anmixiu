#![forbid(unsafe_code)]

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anmixiu_scene::{Point, Rect, Size};
use taffy::{
    TaffyTree,
    geometry::{Rect as TaffyRect, Size as TaffySize},
    prelude::{
        AlignContent as TaffyAlignContent, AlignItems as TaffyAlignItems,
        AvailableSpace as TaffyAvailableSpace, Dimension as TaffyDimension,
        Display as TaffyDisplay, FlexDirection as TaffyFlexDirection,
        LengthPercentage as TaffyLength, NodeId as TaffyNodeId, Style as TaffyStyle,
    },
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayoutNodeId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeasureId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Dimension {
    #[default]
    Auto,
    Points(f32),
    Percent(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    #[must_use]
    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    #[must_use]
    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    #[default]
    Column,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Platform-neutral subset of Flexbox consumed by the internal Taffy adapter.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutStyle {
    pub width: Dimension,
    pub height: Dimension,
    pub min_width: Dimension,
    pub min_height: Dimension,
    pub max_width: Dimension,
    pub max_height: Dimension,
    pub padding: Edges,
    pub gap: f32,
    pub direction: FlexDirection,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub justify_content: Justify,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Dimension,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            width: Dimension::Auto,
            height: Dimension::Auto,
            min_width: Dimension::Auto,
            min_height: Dimension::Auto,
            max_width: Dimension::Auto,
            max_height: Dimension::Auto,
            padding: Edges::default(),
            gap: 0.0,
            direction: FlexDirection::Column,
            align_items: Align::Stretch,
            align_self: None,
            justify_content: Justify::Start,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
        }
    }
}

/// Stable input tree produced from `anmixiu-core` elements. It intentionally contains no
/// Taffy types so the layout implementation can be replaced without changing core contracts.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutNode {
    id: LayoutNodeId,
    style: LayoutStyle,
    measure: Option<MeasureId>,
    children: Vec<Self>,
}

impl LayoutNode {
    #[must_use]
    pub fn new(id: LayoutNodeId) -> Self {
        Self {
            id,
            style: LayoutStyle::default(),
            measure: None,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_style(mut self, style: LayoutStyle) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub fn with_measure(mut self, measure: MeasureId) -> Self {
        self.measure = Some(measure);
        self
    }

    #[must_use]
    pub fn with_child(mut self, child: Self) -> Self {
        self.children.push(child);
        self
    }

    #[must_use]
    pub const fn id(&self) -> LayoutNodeId {
        self.id
    }

    #[must_use]
    pub const fn style(&self) -> &LayoutStyle {
        &self.style
    }

    #[must_use]
    pub const fn measure(&self) -> Option<MeasureId> {
        self.measure
    }

    #[must_use]
    pub fn children(&self) -> &[Self] {
        &self.children
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub size: Size,
    pub scale: f32,
}

impl Viewport {
    #[must_use]
    pub const fn new(size: Size, scale: f32) -> Self {
        Self { size, scale }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LayoutRevisions {
    pub structure: u64,
    pub style: u64,
    pub measure: u64,
}

impl LayoutRevisions {
    #[must_use]
    pub const fn new(structure: u64, style: u64, measure: u64) -> Self {
        Self {
            structure,
            style,
            measure,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutRequest<'a> {
    pub root: &'a LayoutNode,
    pub viewport: Viewport,
    pub revisions: LayoutRevisions,
}

impl<'a> LayoutRequest<'a> {
    #[must_use]
    pub const fn new(root: &'a LayoutNode, viewport: Viewport, revisions: LayoutRevisions) -> Self {
        Self {
            root,
            viewport,
            revisions,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AvailableLength {
    Definite(f32),
    MinContent,
    MaxContent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeasureConstraints {
    pub known_width: Option<f32>,
    pub known_height: Option<f32>,
    pub available_width: AvailableLength,
    pub available_height: AvailableLength,
    pub scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutBox {
    pub id: LayoutNodeId,
    pub bounds: Rect,
    pub parent: Option<LayoutNodeId>,
    pub children: Arc<[LayoutNodeId]>,
    pub order: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutTree {
    root: LayoutNodeId,
    boxes: HashMap<LayoutNodeId, LayoutBox>,
    paint_order: Arc<[LayoutNodeId]>,
}

impl LayoutTree {
    #[must_use]
    pub const fn root(&self) -> LayoutNodeId {
        self.root
    }

    #[must_use]
    pub fn get(&self, id: LayoutNodeId) -> Option<&LayoutBox> {
        self.boxes.get(&id)
    }

    #[must_use]
    pub fn bounds(&self, id: LayoutNodeId) -> Option<Rect> {
        self.get(id).map(|layout_box| layout_box.bounds)
    }

    #[must_use]
    pub fn paint_order(&self) -> &[LayoutNodeId] {
        &self.paint_order
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutCacheStats {
    pub hits: u64,
    pub misses: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutCacheKey {
    root: LayoutNodeId,
    revisions: LayoutRevisions,
    width_bits: u32,
    height_bits: u32,
    scale_bits: u32,
}

impl LayoutCacheKey {
    fn new(request: LayoutRequest<'_>) -> Self {
        Self {
            root: request.root.id,
            revisions: request.revisions,
            width_bits: request.viewport.size.width.to_bits(),
            height_bits: request.viewport.size.height.to_bits(),
            scale_bits: request.viewport.scale.to_bits(),
        }
    }
}

#[derive(Clone, Debug)]
struct CachedLayout {
    key: LayoutCacheKey,
    tree: Arc<LayoutTree>,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum LayoutError {
    #[error("layout node id {0:?} occurs more than once")]
    DuplicateNodeId(LayoutNodeId),
    #[error("measured layout node {0:?} cannot also have children")]
    MeasuredNodeHasChildren(LayoutNodeId),
    #[error(
        "viewport width, height, and scale must be finite and non-negative, with scale greater than zero"
    )]
    InvalidViewport,
    #[error("Taffy layout failed: {0}")]
    Taffy(String),
}

/// One-entry cache for a window's current layout. Any structure, style, measurement,
/// logical-size, or scale change replaces the entry, keeping steady-state memory bounded.
#[derive(Debug, Default)]
pub struct LayoutEngine {
    cached: Option<CachedLayout>,
    stats: LayoutCacheStats,
}

impl LayoutEngine {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cached: None,
            stats: LayoutCacheStats { hits: 0, misses: 0 },
        }
    }

    /// Computes a logical-pixel layout or returns the cached tree for the exact request key.
    ///
    /// Callers must advance the appropriate revision whenever the projected input changes.
    ///
    /// # Errors
    ///
    /// Returns a structured error for an invalid viewport or input tree, or if Taffy rejects
    /// the internally adapted tree.
    pub fn compute(
        &mut self,
        request: LayoutRequest<'_>,
        mut measure: impl FnMut(MeasureId, MeasureConstraints) -> Size,
    ) -> Result<Arc<LayoutTree>, LayoutError> {
        validate_viewport(request.viewport)?;
        let key = LayoutCacheKey::new(request);
        if let Some(cached) = &self.cached
            && cached.key == key
        {
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Ok(Arc::clone(&cached.tree));
        }

        let tree = Arc::new(compute_uncached(request, &mut measure)?);
        self.stats.misses = self.stats.misses.saturating_add(1);
        self.cached = Some(CachedLayout {
            key,
            tree: Arc::clone(&tree),
        });
        Ok(tree)
    }

    #[must_use]
    pub const fn stats(&self) -> LayoutCacheStats {
        self.stats
    }

    #[must_use]
    pub fn cached_entries(&self) -> usize {
        usize::from(self.cached.is_some())
    }

    pub fn invalidate(&mut self) {
        self.cached = None;
    }
}

fn validate_viewport(viewport: Viewport) -> Result<(), LayoutError> {
    let size_valid = viewport.size.width.is_finite()
        && viewport.size.width >= 0.0
        && viewport.size.height.is_finite()
        && viewport.size.height >= 0.0;
    let scale_valid = viewport.scale.is_finite() && viewport.scale > 0.0;
    if size_valid && scale_valid {
        Ok(())
    } else {
        Err(LayoutError::InvalidViewport)
    }
}

fn compute_uncached(
    request: LayoutRequest<'_>,
    measure: &mut impl FnMut(MeasureId, MeasureConstraints) -> Size,
) -> Result<LayoutTree, LayoutError> {
    let node_count = count_nodes(request.root);
    let mut taffy = TaffyTree::<MeasureId>::with_capacity(node_count);
    taffy.disable_rounding();
    let mut node_map = HashMap::with_capacity(node_count);
    let mut seen = HashSet::with_capacity(node_count);
    let root = add_node(
        &mut taffy,
        request.root,
        request.viewport,
        true,
        &mut node_map,
        &mut seen,
    )?;

    let available_space = TaffySize {
        width: TaffyAvailableSpace::Definite(request.viewport.size.width),
        height: TaffyAvailableSpace::Definite(request.viewport.size.height),
    };
    let scale = request.viewport.scale;
    taffy
        .compute_layout_with_measure(root, available_space, |known, available, _, context, _| {
            let measured = context.map_or_else(Size::default, |measure_id| {
                measure(
                    *measure_id,
                    MeasureConstraints {
                        known_width: known.width,
                        known_height: known.height,
                        available_width: from_available(available.width),
                        available_height: from_available(available.height),
                        scale,
                    },
                )
            });
            TaffySize {
                width: known
                    .width
                    .unwrap_or_else(|| finite_non_negative(measured.width)),
                height: known
                    .height
                    .unwrap_or_else(|| finite_non_negative(measured.height)),
            }
        })
        .map_err(|error| LayoutError::Taffy(error.to_string()))?;

    let mut boxes = HashMap::with_capacity(node_count);
    let mut paint_order = Vec::with_capacity(node_count);
    collect_layout(
        &taffy,
        request.root,
        &node_map,
        None,
        Point::default(),
        &mut boxes,
        &mut paint_order,
    )?;
    Ok(LayoutTree {
        root: request.root.id,
        boxes,
        paint_order: paint_order.into(),
    })
}

fn count_nodes(node: &LayoutNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

fn add_node(
    taffy: &mut TaffyTree<MeasureId>,
    node: &LayoutNode,
    viewport: Viewport,
    is_root: bool,
    node_map: &mut HashMap<LayoutNodeId, TaffyNodeId>,
    seen: &mut HashSet<LayoutNodeId>,
) -> Result<TaffyNodeId, LayoutError> {
    if !seen.insert(node.id) {
        return Err(LayoutError::DuplicateNodeId(node.id));
    }
    if node.measure.is_some() && !node.children.is_empty() {
        return Err(LayoutError::MeasuredNodeHasChildren(node.id));
    }

    let children = node
        .children
        .iter()
        .map(|child| add_node(taffy, child, viewport, false, node_map, seen))
        .collect::<Result<Vec<_>, _>>()?;
    let style = into_taffy_style(&node.style, is_root.then_some(viewport.size));
    let taffy_id = if let Some(measure) = node.measure {
        taffy
            .new_leaf_with_context(style, measure)
            .map_err(|error| LayoutError::Taffy(error.to_string()))?
    } else {
        taffy
            .new_with_children(style, &children)
            .map_err(|error| LayoutError::Taffy(error.to_string()))?
    };
    node_map.insert(node.id, taffy_id);
    Ok(taffy_id)
}

fn collect_layout(
    taffy: &TaffyTree<MeasureId>,
    node: &LayoutNode,
    node_map: &HashMap<LayoutNodeId, TaffyNodeId>,
    parent: Option<LayoutNodeId>,
    parent_origin: Point,
    boxes: &mut HashMap<LayoutNodeId, LayoutBox>,
    paint_order: &mut Vec<LayoutNodeId>,
) -> Result<(), LayoutError> {
    let taffy_id = node_map
        .get(&node.id)
        .copied()
        .ok_or_else(|| LayoutError::Taffy("internal node map is incomplete".into()))?;
    let layout = taffy
        .layout(taffy_id)
        .map_err(|error| LayoutError::Taffy(error.to_string()))?;
    let origin = Point::new(
        parent_origin.x + layout.location.x,
        parent_origin.y + layout.location.y,
    );
    let children: Arc<[LayoutNodeId]> = node.children.iter().map(|child| child.id).collect();
    boxes.insert(
        node.id,
        LayoutBox {
            id: node.id,
            bounds: Rect::new(origin, Size::new(layout.size.width, layout.size.height)),
            parent,
            children,
            order: layout.order,
        },
    );
    paint_order.push(node.id);
    for child in &node.children {
        collect_layout(
            taffy,
            child,
            node_map,
            Some(node.id),
            origin,
            boxes,
            paint_order,
        )?;
    }
    Ok(())
}

fn into_taffy_style(style: &LayoutStyle, root_fallback: Option<Size>) -> TaffyStyle {
    let width = root_fallback.map_or(style.width, |size| match style.width {
        Dimension::Auto => Dimension::Points(size.width),
        value => value,
    });
    let height = root_fallback.map_or(style.height, |size| match style.height {
        Dimension::Auto => Dimension::Points(size.height),
        value => value,
    });
    TaffyStyle {
        display: TaffyDisplay::Flex,
        size: TaffySize {
            width: into_dimension(width),
            height: into_dimension(height),
        },
        min_size: TaffySize {
            width: into_dimension(style.min_width),
            height: into_dimension(style.min_height),
        },
        max_size: TaffySize {
            width: into_dimension(style.max_width),
            height: into_dimension(style.max_height),
        },
        padding: TaffyRect {
            left: TaffyLength::length(finite_non_negative(style.padding.left)),
            right: TaffyLength::length(finite_non_negative(style.padding.right)),
            top: TaffyLength::length(finite_non_negative(style.padding.top)),
            bottom: TaffyLength::length(finite_non_negative(style.padding.bottom)),
        },
        gap: TaffySize {
            width: TaffyLength::length(finite_non_negative(style.gap)),
            height: TaffyLength::length(finite_non_negative(style.gap)),
        },
        flex_direction: match style.direction {
            FlexDirection::Row => TaffyFlexDirection::Row,
            FlexDirection::Column => TaffyFlexDirection::Column,
        },
        align_items: Some(into_align(style.align_items)),
        align_self: style.align_self.map(into_align),
        justify_content: Some(into_justify(style.justify_content)),
        flex_grow: finite_non_negative(style.flex_grow),
        flex_shrink: finite_non_negative(style.flex_shrink),
        flex_basis: into_dimension(style.flex_basis),
        ..TaffyStyle::default()
    }
}

fn into_dimension(value: Dimension) -> TaffyDimension {
    match value {
        Dimension::Auto => TaffyDimension::auto(),
        Dimension::Points(value) => TaffyDimension::length(finite_non_negative(value)),
        Dimension::Percent(value) => TaffyDimension::percent(finite_non_negative(value)),
    }
}

fn into_align(value: Align) -> TaffyAlignItems {
    match value {
        Align::Start => TaffyAlignItems::FlexStart,
        Align::Center => TaffyAlignItems::Center,
        Align::End => TaffyAlignItems::FlexEnd,
        Align::Stretch => TaffyAlignItems::Stretch,
    }
}

fn into_justify(value: Justify) -> TaffyAlignContent {
    match value {
        Justify::Start => TaffyAlignContent::FlexStart,
        Justify::Center => TaffyAlignContent::Center,
        Justify::End => TaffyAlignContent::FlexEnd,
        Justify::SpaceBetween => TaffyAlignContent::SpaceBetween,
        Justify::SpaceAround => TaffyAlignContent::SpaceAround,
        Justify::SpaceEvenly => TaffyAlignContent::SpaceEvenly,
    }
}

fn from_available(value: TaffyAvailableSpace) -> AvailableLength {
    match value {
        TaffyAvailableSpace::Definite(value) => AvailableLength::Definite(value),
        TaffyAvailableSpace::MinContent => AvailableLength::MinContent,
        TaffyAvailableSpace::MaxContent => AvailableLength::MaxContent,
    }
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}
