use std::{cell::RefCell, collections::HashSet, rc::Rc, sync::Arc, time::Instant};

use crate::{BuiltFrame, FrameBuilder, InvalidationGuard, PointerTracker, Viewport};
use anmixiu_core::{
    AppEvents, AppHandle, AppStateStore, CursorStyle, ErasedElementHost, GlobalElementId,
    WindowHandle, WindowMountContext, WindowRoot, WindowStateStore,
};
use anmixiu_reactive::{OwnerId, OwnerRegistry};
use anmixiu_render::{FrameOutcome, Renderer, SurfaceSize};
use anmixiu_runtime::AppRuntime;
use anmixiu_scene::{Point, Size};
use winit::{event_loop::EventLoopProxy, window::Window as NativeWindow};

use super::{AppError, UserEvent, send_wake};

use anmixiu_text::FontSpec;

const MAX_RENDER_INVALIDATIONS: usize = 8;

struct DriverState {
    runtime: Rc<AppRuntime>,
    owners: OwnerRegistry,
    owner: OwnerId,
    host: Box<dyn ErasedElementHost>,
    frame_builder: FrameBuilder,
    renderer: Renderer,
    window: Arc<NativeWindow>,
    viewport: Viewport,
    frame: Option<BuiltFrame>,
    pointer: PointerTracker,
    pressed_element: Option<GlobalElementId>,
    needs_frame: bool,
    invalidation_guard: InvalidationGuard,
    stalled: HashSet<OwnerId>,
    last_draw_at: Option<Instant>,
    error: Option<AppError>,
}

pub(super) struct ComponentDriver {
    state: RefCell<DriverState>,
    proxy: EventLoopProxy<UserEvent>,
}

impl ComponentDriver {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        root: WindowRoot,
        app_state: AppStateStore,
        window_state: WindowStateStore,
        font: FontSpec,
        app_events: AppEvents,
        app_handle: AppHandle,
        window_handle: WindowHandle,
        runtime: Rc<AppRuntime>,
        proxy: EventLoopProxy<UserEvent>,
        window: Arc<NativeWindow>,
        viewport: Viewport,
    ) -> Result<Self, AppError> {
        let owners = OwnerRegistry::new();
        let spawner = runtime.ui().spawner(owners.clone());
        let mounted = root.mount(WindowMountContext {
            app_state,
            window_state,
            app_events,
            app_handle,
            window_handle,
            owners: owners.clone(),
            spawner,
        });
        let mut renderer = Renderer::new()?;
        renderer.attach_surface(window.clone(), surface_size(viewport))?;
        Ok(Self {
            state: RefCell::new(DriverState {
                runtime,
                owners,
                owner: mounted.owner,
                host: mounted.host,
                frame_builder: FrameBuilder::new_with_font(font)?,
                renderer,
                window,
                viewport,
                frame: None,
                pointer: PointerTracker::default(),
                pressed_element: None,
                needs_frame: true,
                invalidation_guard: InvalidationGuard::default(),
                stalled: HashSet::new(),
                last_draw_at: None,
                error: None,
            }),
            proxy,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn draw(&self) {
        let mut needs_follow_up = false;
        let result = (|| -> Result<(), AppError> {
            let mut state = self.state.borrow_mut();
            if state.error.is_some() {
                return Ok(());
            }
            let now = Instant::now();
            let delta_seconds = state
                .last_draw_at
                .replace(now)
                .map_or(1.0 / 60.0, |previous| {
                    previous.elapsed().as_secs_f32().clamp(1.0 / 240.0, 0.1)
                });
            if state
                .frame
                .as_ref()
                .is_some_and(|frame| frame.advance_scroll(delta_seconds))
            {
                state.frame_builder.note_scrolled();
                state.needs_frame = true;
                needs_follow_up = true;
            }
            let dirty = state.owners.take_dirty();
            for owner in &dirty {
                if state.stalled.remove(owner) {
                    state.invalidation_guard.reset(*owner);
                }
            }
            let renderable_dirty = dirty
                .iter()
                .copied()
                .filter(|owner| !state.stalled.contains(owner) && state.host.contains_owner(*owner))
                .collect::<Vec<_>>();
            let rerender = state.frame.is_none() || !renderable_dirty.is_empty();
            if rerender {
                if state.frame.is_none() {
                    state
                        .host
                        .render()
                        .map_err(|error| AppError::Element(error.to_string()))?;
                } else {
                    state
                        .host
                        .render_dirty(&renderable_dirty)
                        .map_err(|error| AppError::Element(error.to_string()))?;
                }
                state.needs_frame = true;
            }
            if !state.needs_frame {
                return Ok(());
            }
            let element = state
                .host
                .element_snapshot()
                .expect("a requested frame has a rendered root");
            let (logical_width, logical_height) = state.viewport.logical_size();
            let scale = state.viewport.scale();
            let mut frame = state.frame_builder.build(
                element.as_ref(),
                Size::new(logical_width, logical_height),
                scale,
            )?;
            let hover_point = state.pointer.is_inside().then(|| {
                let (x, y) = state.pointer.position();
                Point::new(x, y)
            });
            if state.frame_builder.update_hover(&frame, hover_point) {
                frame = state.frame_builder.build(
                    element.as_ref(),
                    Size::new(logical_width, logical_height),
                    scale,
                )?;
            }
            let surface = surface_size(state.viewport);
            let outcome = state
                .renderer
                .render_surface(&frame.scene, surface, scale)?;
            state.frame = Some(frame);
            match outcome {
                FrameOutcome::Presented => {
                    state.needs_frame = false;
                    state.host.did_paint();
                }
                FrameOutcome::DrawableUnavailable { retry_immediately }
                | FrameOutcome::SurfaceOutOfDate { retry_immediately } => {
                    state.needs_frame = true;
                    needs_follow_up |= retry_immediately;
                    if matches!(outcome, FrameOutcome::SurfaceOutOfDate { .. }) {
                        state.renderer.resize_surface(surface)?;
                    }
                }
                FrameOutcome::SurfaceLost => {
                    let window = state.window.clone();
                    state.renderer.attach_surface(window, surface)?;
                    state.needs_frame = true;
                    needs_follow_up = true;
                }
            }

            let animating_sites = state.owners.take_animating_with_sites();
            let animating = animating_sites
                .iter()
                .map(|(owner, _)| *owner)
                .collect::<Vec<_>>();
            let self_invalidators =
                anonymous_self_invalidators(&state.owners.dirty_snapshot(), &animating);
            let runaway = state
                .invalidation_guard
                .advance(&self_invalidators, MAX_RENDER_INVALIDATIONS);
            if !runaway.is_empty() {
                #[cfg(debug_assertions)]
                return Err(AppError::RenderLoop(
                    runaway
                        .iter()
                        .map(|invalidation| invalidation.streak)
                        .max()
                        .unwrap_or(MAX_RENDER_INVALIDATIONS),
                ));
                #[cfg(not(debug_assertions))]
                for invalidation in &runaway {
                    if !state.owners.clear_dirty(invalidation.owner) {
                        tracing::warn!(owner = ?invalidation.owner, "runaway owner already clean");
                    }
                    state.stalled.insert(invalidation.owner);
                    state.invalidation_guard.reset(invalidation.owner);
                }
            }
            if !animating.is_empty() || state.owners.dirty_len() != 0 {
                needs_follow_up = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
            return;
        }
        if needs_follow_up {
            send_wake(&self.proxy);
        }
    }

    pub(super) fn resize(&self, viewport: Viewport) {
        let result = (|| -> Result<bool, AppError> {
            let mut state = self.state.borrow_mut();
            if state.viewport == viewport {
                return Ok(false);
            }
            state.renderer.resize_surface(surface_size(viewport))?;
            state.viewport = viewport;
            state.needs_frame = true;
            Ok(true)
        })();
        match result {
            Ok(true) => send_wake(&self.proxy),
            Ok(false) => {}
            Err(error) => self.fail(error),
        }
    }

    pub(super) fn pointer_moved(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let changed = {
            let DriverState {
                frame_builder,
                frame,
                ..
            } = &mut *state;
            frame
                .as_ref()
                .is_some_and(|frame| frame_builder.update_hover(frame, Some(point)))
        };
        if changed {
            state.needs_frame = true;
        }
        drop(state);
        if changed {
            send_wake(&self.proxy);
        }
    }

    pub(super) fn pointer_position(&self) -> Point {
        let (x, y) = self.state.borrow().pointer.position();
        Point::new(x, y)
    }

    pub(super) fn cursor_style_at(&self, point: Point) -> CursorStyle {
        let state = self.state.borrow();
        state.frame.as_ref().map_or(CursorStyle::Default, |frame| {
            frame
                .scene
                .hit_test(point)
                .map_or(CursorStyle::Default, |hit| frame.cursor_style(hit))
        })
    }

    pub(super) fn pointer_exited(&self) {
        let mut state = self.state.borrow_mut();
        state.pointer.exit();
        let changed = {
            let DriverState {
                frame_builder,
                frame,
                ..
            } = &mut *state;
            frame
                .as_ref()
                .is_some_and(|frame| frame_builder.update_hover(frame, None))
        };
        if changed {
            state.needs_frame = true;
        }
        drop(state);
        if changed {
            send_wake(&self.proxy);
        }
    }

    pub(super) fn pointer_down(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let hit = state
            .frame
            .as_ref()
            .and_then(|frame| frame.click_target_at(point));
        let focused = hit.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.global_id(hit).cloned())
        });
        state.pressed_element.clone_from(&focused);
        state.pointer.press(hit.map(|hit| hit.0));
        let focus_changed = {
            let DriverState {
                frame_builder,
                frame,
                ..
            } = &mut *state;
            frame
                .as_ref()
                .is_some_and(|frame| frame_builder.focus_at(frame, point))
        };
        if focus_changed {
            state.needs_frame = true;
        }
        drop(state);
        if focus_changed {
            send_wake(&self.proxy);
        }
    }

    pub(super) fn pointer_up(&self, point: Point) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let target = state
            .frame
            .as_ref()
            .and_then(|frame| frame.click_target_at(point));
        let current = target.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.global_id(hit).cloned())
        });
        state.pointer.release(target.map(|hit| hit.0));
        let pressed = state.pressed_element.take();
        let clicked = (pressed.is_some() && pressed == current)
            .then_some(target)
            .flatten();
        let handler = clicked.and_then(|hit| {
            state
                .frame
                .as_ref()
                .and_then(|frame| frame.handler(hit).cloned())
        });
        if let Some(handler) = handler
            && let Some(future) = handler.invoke()
        {
            let owner = clicked
                .and_then(|hit| state.frame.as_ref()?.handler_owner(hit))
                .unwrap_or(state.owner);
            if let Err(error) = state.runtime.ui().spawn(&state.owners, owner, future) {
                tracing::error!(%error, ?owner, "async click handler could not be scheduled");
            }
        }
        let dirty = state.owners.dirty_len() != 0;
        drop(state);
        if dirty {
            send_wake(&self.proxy);
        }
    }

    pub(super) fn scroll(&self, point: Point, delta_x: f32, delta_y: f32) {
        let mut state = self.state.borrow_mut();
        state.pointer.update_position(point.x, point.y);
        let consumed = state
            .frame
            .as_ref()
            .is_some_and(|frame| frame.scroll_at_axes(point, delta_x, delta_y));
        if consumed {
            state.frame_builder.note_scrolled();
            state.needs_frame = true;
            state.last_draw_at = None;
        }
        drop(state);
        if consumed {
            send_wake(&self.proxy);
        }
    }

    pub(super) fn shutdown(&self) {
        self.state.borrow_mut().host.unmount();
    }

    pub(super) fn is_dirty(&self) -> bool {
        let state = self.state.borrow();
        state.needs_frame || state.owners.dirty_len() != 0
    }

    pub(super) fn take_error(&self) -> Option<AppError> {
        self.state.borrow_mut().error.take()
    }

    fn fail(&self, error: AppError) {
        let mut state = self.state.borrow_mut();
        if state.error.is_none() {
            eprintln!("Anmixiu stopped after an unrecoverable error: {error}");
            state.error = Some(error);
        }
        drop(state);
        send_wake(&self.proxy);
    }
}

fn surface_size(viewport: Viewport) -> SurfaceSize {
    let (width, height) = viewport.physical_size();
    SurfaceSize::new(width.max(1), height.max(1)).expect("clamped dimensions are non-zero")
}

fn anonymous_self_invalidators(dirty: &[OwnerId], animating: &[OwnerId]) -> Vec<OwnerId> {
    let animating: HashSet<OwnerId> = animating.iter().copied().collect();
    dirty
        .iter()
        .copied()
        .filter(|owner| !animating.contains(owner))
        .collect()
}
