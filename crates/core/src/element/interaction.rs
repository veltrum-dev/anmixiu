use std::{cell::RefCell, fmt, future::Future, pin::Pin, rc::Rc};

use anmixiu_reactive::OwnerId;

use super::{id::ElementId, stateful::Stateful, style::StyleRefinement};

type LocalFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

enum ClickKind {
    Sync(RefCell<Box<dyn FnMut()>>),
    Async(RefCell<Box<dyn FnMut() -> LocalFuture>>),
    Bound {
        handler: Rc<ClickKind>,
        owner: OwnerId,
    },
}

#[derive(Clone)]
pub struct ClickHandler(Rc<ClickKind>);

impl fmt::Debug for ClickHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClickHandler(..)")
    }
}

struct HoverKind(RefCell<Box<dyn FnMut(bool)>>);

#[derive(Clone)]
pub struct HoverHandler(Rc<HoverKind>);

impl fmt::Debug for HoverHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HoverHandler(..)")
    }
}

impl HoverHandler {
    pub fn invoke(&self, hovered: bool) {
        (self.0.0.borrow_mut())(hovered);
    }
}

impl ClickHandler {
    #[must_use]
    pub fn invoke(&self) -> Option<LocalFuture> {
        match self.0.as_ref() {
            ClickKind::Sync(callback) => {
                (callback.borrow_mut())();
                None
            }
            ClickKind::Async(callback) => Some((callback.borrow_mut())()),
            ClickKind::Bound { handler, .. } => Self(handler.clone()).invoke(),
        }
    }

    pub(crate) fn bind_owner(&mut self, owner: OwnerId) {
        if self.owner_id() == Some(owner) {
            return;
        }
        let handler = match self.0.as_ref() {
            ClickKind::Bound { handler, .. } => handler.clone(),
            ClickKind::Sync(_) | ClickKind::Async(_) => self.0.clone(),
        };
        self.0 = Rc::new(ClickKind::Bound { handler, owner });
    }

    #[doc(hidden)]
    #[must_use]
    pub fn owner_id(&self) -> Option<OwnerId> {
        match self.0.as_ref() {
            ClickKind::Bound { owner, .. } => Some(*owner),
            ClickKind::Sync(_) | ClickKind::Async(_) => None,
        }
    }
}

pub(crate) mod sealed {
    pub trait Marker {}

    pub struct Sync;
    pub struct Async;

    impl Marker for Sync {}
    impl Marker for Async {}
}

pub trait IntoClickHandler<M: sealed::Marker>: 'static {
    fn into_click_handler(self) -> ClickHandler;
}

pub trait IntoHoverHandler: 'static {
    fn into_hover_handler(self) -> HoverHandler;
}

impl<F> IntoHoverHandler for F
where
    F: FnMut(bool) + 'static,
{
    fn into_hover_handler(self) -> HoverHandler {
        HoverHandler(Rc::new(HoverKind(RefCell::new(Box::new(self)))))
    }
}

impl<F> IntoClickHandler<sealed::Sync> for F
where
    F: FnMut() + 'static,
{
    fn into_click_handler(self) -> ClickHandler {
        ClickHandler(Rc::new(ClickKind::Sync(RefCell::new(Box::new(self)))))
    }
}

impl<F, Fut> IntoClickHandler<sealed::Async> for F
where
    F: FnMut() -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
{
    fn into_click_handler(mut self) -> ClickHandler {
        let callback = move || -> LocalFuture { Box::pin(self()) };
        ClickHandler(Rc::new(ClickKind::Async(RefCell::new(Box::new(callback)))))
    }
}

/// Identity capability for elements that participate in interaction.
pub trait InteractiveElement: Sized {
    #[doc(hidden)]
    fn assign_element_id(&mut self, id: ElementId);

    fn element_id(&self) -> Option<&ElementId>;

    #[doc(hidden)]
    fn assign_click_handler(&mut self, handler: ClickHandler);

    fn click_handler(&self) -> Option<&ClickHandler>;

    #[doc(hidden)]
    fn assign_hover_style(&mut self, style: StyleRefinement);

    fn hover_style(&self) -> Option<&StyleRefinement>;

    #[doc(hidden)]
    fn assign_hover_handler(&mut self, handler: HoverHandler);

    fn hover_handler(&self) -> Option<&HoverHandler>;

    #[must_use]
    fn hover(mut self, refine: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        let style = self.hover_style().cloned().unwrap_or_default();
        self.assign_hover_style(refine(style));
        self
    }

    #[must_use]
    fn id(self, id: impl Into<ElementId>) -> Stateful<Self> {
        Stateful::new(self, id.into())
    }
}

/// Stateful interactions unlocked after [`InteractiveElement::id`].
///
/// ```compile_fail
/// use anmixiu_core::{StatefulInteractiveElement, button};
///
/// // A stable ElementId is required before stateful multi-phase interaction.
/// let _ = button("Save").on_click(|| {});
/// ```
pub trait StatefulInteractiveElement: Sized {
    #[must_use]
    fn on_click<H, M>(self, handler: H) -> Self
    where
        M: sealed::Marker,
        H: IntoClickHandler<M>;

    #[must_use]
    fn on_hover_change<H>(self, handler: H) -> Self
    where
        H: IntoHoverHandler;
}

impl<E: InteractiveElement> InteractiveElement for Stateful<E> {
    fn assign_element_id(&mut self, id: ElementId) {
        self.id = id;
    }

    fn element_id(&self) -> Option<&ElementId> {
        Some(&self.id)
    }

    fn assign_click_handler(&mut self, handler: ClickHandler) {
        self.inner_mut().assign_click_handler(handler);
    }

    fn click_handler(&self) -> Option<&ClickHandler> {
        self.inner().click_handler()
    }

    fn assign_hover_style(&mut self, style: StyleRefinement) {
        self.inner_mut().assign_hover_style(style);
    }

    fn hover_style(&self) -> Option<&StyleRefinement> {
        self.inner().hover_style()
    }

    fn assign_hover_handler(&mut self, handler: HoverHandler) {
        self.inner_mut().assign_hover_handler(handler);
    }

    fn hover_handler(&self) -> Option<&HoverHandler> {
        self.inner().hover_handler()
    }
}

impl<E: InteractiveElement> StatefulInteractiveElement for Stateful<E> {
    fn on_click<H, M>(mut self, handler: H) -> Self
    where
        M: sealed::Marker,
        H: IntoClickHandler<M>,
    {
        self.inner_mut()
            .assign_click_handler(handler.into_click_handler());
        self
    }

    fn on_hover_change<H>(mut self, handler: H) -> Self
    where
        H: IntoHoverHandler,
    {
        self.inner_mut()
            .assign_hover_handler(handler.into_hover_handler());
        self
    }
}
