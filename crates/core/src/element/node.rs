use crate::SharedString;

use super::{
    id::ElementId,
    interaction::{ClickHandler, HoverHandler, InteractiveElement},
    style::{Style, StyleRefinement, Styled},
    traits::{Element, ParentElement},
};

#[derive(Clone, Debug)]
pub(crate) enum ElementKind {
    Div,
    Text(SharedString),
    Button(SharedString),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ElementBase {
    pub style: Style,
    pub id: Option<ElementId>,
    pub click: Option<ClickHandler>,
    pub hover_handler: Option<HoverHandler>,
    pub hover: Option<StyleRefinement>,
    pub scroll: Option<super::scroll::ScrollHandle>,
}

/// Type-erased node shared by the core, layout, and platform crates.
///
/// Public builders return concrete element types; application code should not construct this
/// projection directly.
#[derive(Clone, Debug)]
pub struct ElementNode {
    kind: ElementKind,
    base: ElementBase,
    children: Vec<Self>,
}

impl ElementNode {
    pub(crate) const fn new(kind: ElementKind, base: ElementBase, children: Vec<Self>) -> Self {
        Self {
            kind,
            base,
            children,
        }
    }

    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self.kind {
            ElementKind::Div => "div",
            ElementKind::Text(_) => "text",
            ElementKind::Button(_) => "button",
        }
    }

    #[must_use]
    pub fn text_content(&self) -> Option<&str> {
        match &self.kind {
            ElementKind::Text(value) | ElementKind::Button(value) => Some(value),
            ElementKind::Div => None,
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn text_value(&self) -> Option<&SharedString> {
        match &self.kind {
            ElementKind::Text(value) | ElementKind::Button(value) => Some(value),
            ElementKind::Div => None,
        }
    }

    /// The scroll handle if this element is a scroll container, else `None`.
    #[doc(hidden)]
    #[must_use]
    pub fn scroll_handle(&self) -> Option<&super::scroll::ScrollHandle> {
        self.base.scroll.as_ref()
    }

    #[must_use]
    pub fn hit_test<F>(&self, x: f32, y: f32, bounds: F) -> Option<HitNode<'_>>
    where
        F: Fn(NodeId) -> Option<(f32, f32, f32, f32)>,
    {
        fn visit<'a, F>(
            element: &'a ElementNode,
            next_id: &mut usize,
            x: f32,
            y: f32,
            bounds: &F,
        ) -> Option<HitNode<'a>>
        where
            F: Fn(NodeId) -> Option<(f32, f32, f32, f32)>,
        {
            let own_id = NodeId(*next_id);
            *next_id += 1;
            let mut children = Vec::with_capacity(element.children.len());
            for child in &element.children {
                let start = *next_id;
                advance_ids(child, next_id);
                children.push((child, start));
            }
            for (child, start) in children.into_iter().rev() {
                let mut child_id = start;
                if let Some(hit) = visit(child, &mut child_id, x, y, bounds) {
                    return Some(hit);
                }
            }
            let (left, top, width, height) = bounds(own_id)?;
            // Half-open bounds, matching `anmixiu_scene::Rect::contains`: a point exactly on the
            // right/bottom edge belongs to the next pixel/sibling, not this element. Using closed
            // bounds here would let the tree walk and the rendered scene disagree on edge hits.
            let contains = x >= left && y >= top && x < left + width && y < top + height;
            let accepts_pointer =
                element.base.click.is_some() || matches!(element.kind, ElementKind::Button(_));
            (contains && accepts_pointer).then_some(HitNode {
                id: own_id,
                element,
            })
        }

        fn advance_ids(element: &ElementNode, next_id: &mut usize) {
            *next_id += 1;
            for child in &element.children {
                advance_ids(child, next_id);
            }
        }

        visit(self, &mut 0, x, y, &bounds)
    }
}

impl Styled for ElementNode {
    fn style(&mut self) -> &mut Style {
        &mut self.base.style
    }

    fn style_ref(&self) -> &Style {
        &self.base.style
    }
}

impl ParentElement for ElementNode {
    fn child_nodes(&mut self) -> &mut Vec<ElementNode> {
        &mut self.children
    }

    fn children_ref(&self) -> &[ElementNode] {
        &self.children
    }
}

impl InteractiveElement for ElementNode {
    fn assign_element_id(&mut self, id: ElementId) {
        self.base.id = Some(id);
    }

    fn element_id(&self) -> Option<&ElementId> {
        self.base.id.as_ref()
    }

    fn assign_click_handler(&mut self, handler: ClickHandler) {
        self.base.click = Some(handler);
    }

    fn click_handler(&self) -> Option<&ClickHandler> {
        self.base.click.as_ref()
    }

    fn assign_hover_style(&mut self, style: StyleRefinement) {
        self.base.hover = Some(style);
    }

    fn hover_style(&self) -> Option<&StyleRefinement> {
        self.base.hover.as_ref()
    }

    fn assign_hover_handler(&mut self, handler: HoverHandler) {
        self.base.hover_handler = Some(handler);
    }

    fn hover_handler(&self) -> Option<&HoverHandler> {
        self.base.hover_handler.as_ref()
    }
}

impl Element for ElementNode {
    fn into_element_node(self) -> ElementNode {
        self
    }
}

/// A dense current-frame index used only for layout and hit-test projection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(usize);

impl NodeId {
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HitNode<'a> {
    id: NodeId,
    element: &'a ElementNode,
}

impl<'a> HitNode<'a> {
    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }

    #[must_use]
    pub fn text_content(&self) -> Option<&'a str> {
        self.element.text_content()
    }

    #[must_use]
    pub fn click_handler(&self) -> Option<&'a ClickHandler> {
        self.element.base.click.as_ref()
    }
}
