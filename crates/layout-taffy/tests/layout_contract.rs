use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use anmixiu_layout_taffy::{
    Align, Dimension, Edges, FlexDirection, Justify, LayoutEngine, LayoutError, LayoutNode,
    LayoutNodeId, LayoutRequest, LayoutRevisions, LayoutStyle, MeasureId, Viewport,
};
use anmixiu_scene::Size;

fn points(value: f32) -> Dimension {
    Dimension::Points(value)
}

fn fixed(width: f32, height: f32) -> LayoutStyle {
    LayoutStyle {
        width: points(width),
        height: points(height),
        ..LayoutStyle::default()
    }
}

fn request(root: &LayoutNode, width: f32, height: f32) -> LayoutRequest<'_> {
    LayoutRequest::new(
        root,
        Viewport::new(Size::new(width, height), 1.0),
        LayoutRevisions::new(1, 1, 1),
    )
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.001,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn row_flex_applies_padding_gap_alignment_and_fixed_child_sizes() {
    let root = LayoutNode::new(LayoutNodeId(1))
        .with_style(LayoutStyle {
            width: points(300.0),
            height: points(100.0),
            padding: Edges::all(10.0),
            gap: 10.0,
            direction: FlexDirection::Row,
            align_items: Align::Center,
            ..LayoutStyle::default()
        })
        .with_child(LayoutNode::new(LayoutNodeId(2)).with_style(fixed(50.0, 20.0)))
        .with_child(LayoutNode::new(LayoutNodeId(3)).with_style(fixed(40.0, 30.0)));
    let mut engine = LayoutEngine::new();

    let tree = engine
        .compute(request(&root, 300.0, 100.0), |_, _| Size::default())
        .unwrap();

    assert_eq!(
        tree.bounds(LayoutNodeId(1)).unwrap().size,
        Size::new(300.0, 100.0)
    );
    assert_close(tree.bounds(LayoutNodeId(2)).unwrap().origin.x, 10.0);
    assert_close(tree.bounds(LayoutNodeId(2)).unwrap().origin.y, 40.0);
    assert_close(tree.bounds(LayoutNodeId(3)).unwrap().origin.x, 70.0);
    assert_close(tree.bounds(LayoutNodeId(3)).unwrap().origin.y, 35.0);
}

#[test]
fn column_flex_honors_justify_end() {
    let root = LayoutNode::new(LayoutNodeId(1))
        .with_style(LayoutStyle {
            width: points(100.0),
            height: points(100.0),
            direction: FlexDirection::Column,
            justify_content: Justify::End,
            ..LayoutStyle::default()
        })
        .with_child(LayoutNode::new(LayoutNodeId(2)).with_style(fixed(10.0, 20.0)))
        .with_child(LayoutNode::new(LayoutNodeId(3)).with_style(fixed(10.0, 30.0)));
    let mut engine = LayoutEngine::new();

    let tree = engine
        .compute(request(&root, 100.0, 100.0), |_, _| Size::default())
        .unwrap();

    assert_close(tree.bounds(LayoutNodeId(2)).unwrap().origin.y, 50.0);
    assert_close(tree.bounds(LayoutNodeId(3)).unwrap().origin.y, 70.0);
}

#[test]
fn text_measurement_receives_constraints_and_scale() {
    let root = LayoutNode::new(LayoutNodeId(1))
        .with_style(LayoutStyle {
            width: points(100.0),
            direction: FlexDirection::Column,
            ..LayoutStyle::default()
        })
        .with_child(LayoutNode::new(LayoutNodeId(2)).with_measure(MeasureId(22)));
    let mut engine = LayoutEngine::new();
    let constraints_seen = RefCell::new(Vec::new());
    let request = LayoutRequest::new(
        &root,
        Viewport::new(Size::new(100.0, 80.0), 2.0),
        LayoutRevisions::new(1, 1, 4),
    );

    let tree = engine
        .compute(request, |measure_id, constraints| {
            assert_eq!(measure_id, MeasureId(22));
            assert_close(constraints.scale, 2.0);
            constraints_seen.borrow_mut().push(constraints);
            Size::new(72.0, 24.0)
        })
        .unwrap();

    let constraints_seen = constraints_seen.borrow();
    assert!(!constraints_seen.is_empty());
    assert!(constraints_seen.iter().all(|constraints| {
        constraints.known_width == Some(100.0)
            || constraints.available_width == anmixiu_layout_taffy::AvailableLength::Definite(100.0)
    }));
    assert_eq!(
        tree.bounds(LayoutNodeId(2)).unwrap().size,
        Size::new(100.0, 24.0)
    );
}

#[test]
fn sibling_styles_remain_isolated() {
    let root = LayoutNode::new(LayoutNodeId(1))
        .with_style(LayoutStyle {
            width: points(200.0),
            height: points(50.0),
            direction: FlexDirection::Row,
            ..LayoutStyle::default()
        })
        .with_child(LayoutNode::new(LayoutNodeId(2)).with_style(fixed(30.0, 10.0)))
        .with_child(LayoutNode::new(LayoutNodeId(3)).with_style(fixed(70.0, 20.0)));
    let mut engine = LayoutEngine::new();

    let tree = engine
        .compute(request(&root, 200.0, 50.0), |_, _| Size::default())
        .unwrap();

    assert_eq!(
        tree.bounds(LayoutNodeId(2)).unwrap().size,
        Size::new(30.0, 10.0)
    );
    assert_eq!(
        tree.bounds(LayoutNodeId(3)).unwrap().size,
        Size::new(70.0, 20.0)
    );
    assert_eq!(root.children()[0].style().width, points(30.0));
    assert_eq!(root.children()[1].style().width, points(70.0));
}

#[test]
fn identical_key_reuses_layout_without_measuring_again() {
    let root = LayoutNode::new(LayoutNodeId(1))
        .with_style(fixed(100.0, 40.0))
        .with_child(LayoutNode::new(LayoutNodeId(2)).with_measure(MeasureId(2)));
    let mut engine = LayoutEngine::new();
    let calls = Cell::new(0);
    let first = engine
        .compute(request(&root, 100.0, 40.0), |_, _| {
            calls.set(calls.get() + 1);
            Size::new(10.0, 10.0)
        })
        .unwrap();
    let calls_after_first = calls.get();
    let second = engine
        .compute(request(&root, 100.0, 40.0), |_, _| {
            calls.set(calls.get() + 1);
            Size::new(20.0, 20.0)
        })
        .unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(calls.get(), calls_after_first);
    assert_eq!(engine.stats().hits, 1);
    assert_eq!(engine.cached_entries(), 1);
}

#[test]
fn every_declared_revision_invalidates_layout() {
    let root = LayoutNode::new(LayoutNodeId(1)).with_style(fixed(100.0, 40.0));
    let mut engine = LayoutEngine::new();
    let viewport = Viewport::new(Size::new(100.0, 40.0), 1.0);
    let revisions = [
        LayoutRevisions::new(1, 1, 1),
        LayoutRevisions::new(2, 1, 1),
        LayoutRevisions::new(2, 2, 1),
        LayoutRevisions::new(2, 2, 2),
    ];
    let mut previous = None;

    for revision in revisions {
        let tree = engine
            .compute(LayoutRequest::new(&root, viewport, revision), |_, _| {
                Size::default()
            })
            .unwrap();
        if let Some(previous) = previous {
            assert!(!Arc::ptr_eq(&previous, &tree));
        }
        previous = Some(tree);
    }
    assert_eq!(engine.stats().misses, 4);
    assert_eq!(
        engine.cached_entries(),
        1,
        "the cache has a hard one-tree capacity"
    );
}

#[test]
fn resize_and_scale_are_part_of_the_invalidation_key() {
    let root = LayoutNode::new(LayoutNodeId(1));
    let mut engine = LayoutEngine::new();
    let revisions = LayoutRevisions::new(1, 1, 1);
    let first = engine
        .compute(
            LayoutRequest::new(
                &root,
                Viewport::new(Size::new(320.0, 200.0), 1.0),
                revisions,
            ),
            |_, _| Size::default(),
        )
        .unwrap();
    let resized = engine
        .compute(
            LayoutRequest::new(
                &root,
                Viewport::new(Size::new(640.0, 400.0), 1.0),
                revisions,
            ),
            |_, _| Size::default(),
        )
        .unwrap();
    let scaled = engine
        .compute(
            LayoutRequest::new(
                &root,
                Viewport::new(Size::new(640.0, 400.0), 2.0),
                revisions,
            ),
            |_, _| Size::default(),
        )
        .unwrap();

    assert_eq!(
        first.bounds(LayoutNodeId(1)).unwrap().size,
        Size::new(320.0, 200.0)
    );
    assert_eq!(
        resized.bounds(LayoutNodeId(1)).unwrap().size,
        Size::new(640.0, 400.0)
    );
    assert_eq!(
        scaled.bounds(LayoutNodeId(1)).unwrap().size,
        Size::new(640.0, 400.0)
    );
    assert!(!Arc::ptr_eq(&resized, &scaled));
}

#[test]
fn duplicate_node_ids_are_a_structured_error() {
    let root = LayoutNode::new(LayoutNodeId(7)).with_child(LayoutNode::new(LayoutNodeId(7)));
    let mut engine = LayoutEngine::new();

    let error = engine
        .compute(request(&root, 100.0, 100.0), |_, _| Size::default())
        .unwrap_err();

    assert_eq!(error, LayoutError::DuplicateNodeId(LayoutNodeId(7)));
}

#[test]
fn cached_tree_remains_alive_for_consumers_after_engine_replaces_it() {
    let root = Rc::new(LayoutNode::new(LayoutNodeId(1)));
    let mut engine = LayoutEngine::new();
    let first = engine
        .compute(request(&root, 100.0, 100.0), |_, _| Size::default())
        .unwrap();
    let first_bounds = first.bounds(LayoutNodeId(1));
    engine
        .compute(request(&root, 200.0, 200.0), |_, _| Size::default())
        .unwrap();

    assert_eq!(first.bounds(LayoutNodeId(1)), first_bounds);
    assert_eq!(engine.cached_entries(), 1);
}

#[test]
fn paint_order_keeps_each_parent_before_its_descendants() {
    let root = LayoutNode::new(LayoutNodeId(1))
        .with_child(LayoutNode::new(LayoutNodeId(2)).with_style(fixed(20.0, 20.0)))
        .with_child(
            LayoutNode::new(LayoutNodeId(3))
                .with_child(LayoutNode::new(LayoutNodeId(4)).with_style(fixed(20.0, 20.0))),
        )
        .with_child(
            LayoutNode::new(LayoutNodeId(5))
                .with_child(LayoutNode::new(LayoutNodeId(6)).with_style(fixed(20.0, 20.0)))
                .with_child(LayoutNode::new(LayoutNodeId(7)).with_style(fixed(20.0, 20.0))),
        )
        .with_child(LayoutNode::new(LayoutNodeId(8)).with_style(fixed(20.0, 20.0)));
    let mut engine = LayoutEngine::new();
    let tree = engine
        .compute(request(&root, 100.0, 100.0), |_, _| Size::default())
        .unwrap();

    assert_eq!(
        tree.paint_order(),
        &[
            LayoutNodeId(1),
            LayoutNodeId(2),
            LayoutNodeId(3),
            LayoutNodeId(4),
            LayoutNodeId(5),
            LayoutNodeId(6),
            LayoutNodeId(7),
            LayoutNodeId(8),
        ],
        "painting follows tree preorder so descendants cannot be covered by a late parent"
    );
}
