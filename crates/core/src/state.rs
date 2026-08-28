use std::{
    any::{Any, TypeId, type_name},
    cell::RefCell,
    collections::HashMap,
    ops::Deref,
    rc::{Rc, Weak},
};

#[derive(Debug)]
pub struct State<T: 'static>(pub Rc<T>);

impl<T: 'static> Clone for State<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: 'static> Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Default)]
struct StateStore {
    values: Rc<RefCell<HashMap<TypeId, Rc<dyn Any>>>>,
}

impl StateStore {
    fn with<T: 'static>(self, state: T) -> Self {
        self.values
            .borrow_mut()
            .insert(TypeId::of::<T>(), Rc::new(state));
        self
    }

    fn get<T: 'static>(&self) -> Option<State<T>> {
        let value = self.values.borrow().get(&TypeId::of::<T>())?.clone();
        Rc::downcast::<T>(value).ok().map(State)
    }

    fn weak<T: 'static>(&self) -> Option<Weak<T>> {
        self.get::<T>().map(|value| Rc::downgrade(&value.0))
    }
}

#[derive(Clone, Default)]
pub struct AppStateStore(StateStore);

impl AppStateStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with<T: 'static>(self, state: T) -> Self {
        Self(self.0.with(state))
    }

    #[must_use]
    pub fn weak<T: 'static>(&self) -> Option<Weak<T>> {
        self.0.weak()
    }

    pub(crate) fn get<T: 'static>(&self) -> Option<State<T>> {
        self.0.get()
    }
}

#[derive(Clone, Default)]
pub struct WindowStateStore(StateStore);

impl WindowStateStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with<T: 'static>(self, state: T) -> Self {
        Self(self.0.with(state))
    }

    #[must_use]
    pub fn weak<T: 'static>(&self) -> Option<Weak<T>> {
        self.0.weak()
    }

    pub(crate) fn get<T: 'static>(&self) -> Option<State<T>> {
        self.0.get()
    }
}

pub(crate) fn required_state<T: 'static>(
    app: &AppStateStore,
    window: &WindowStateStore,
) -> State<T> {
    window.get().or_else(|| app.get()).unwrap_or_else(|| {
        panic!(
            "Anmixiu state `{}` is missing from both the window and application stores",
            type_name::<T>()
        )
    })
}
