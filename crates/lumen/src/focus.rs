//! What keyboard and pointer focus point at.
//!
//! Wayland windows get focus through their wl_surface. X11 windows need more:
//! the X server must also be told which X window holds the input focus
//! (SetInputFocus, or WM_TAKE_FOCUS for clients that manage focus themselves).
//! smithay's `X11Surface` keyboard target does that and then forwards to the
//! wl_surface Xwayland created for the window, so X11 windows are focused
//! through it. Pointer input only ever needs the surface.

use std::borrow::Cow;

use smithay::backend::input::KeyState;
use smithay::desktop::PopupKind;
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent, PointerTarget,
    RelativeMotionEvent,
};
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Serial};
use smithay::wayland::seat::WaylandFocus;
use smithay::xwayland::X11Surface;

use crate::state::Lumen;

#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    Surface(WlSurface),
    X11(X11Surface),
}

impl FocusTarget {
    /// The X11 window behind this focus, if it is one.
    pub fn x11(&self) -> Option<&X11Surface> {
        match self {
            FocusTarget::X11(x) => Some(x),
            FocusTarget::Surface(_) => None,
        }
    }
}

impl From<WlSurface> for FocusTarget {
    fn from(s: WlSurface) -> FocusTarget {
        FocusTarget::Surface(s)
    }
}

impl From<PopupKind> for FocusTarget {
    fn from(p: PopupKind) -> FocusTarget {
        FocusTarget::Surface(p.wl_surface().clone())
    }
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        match self {
            FocusTarget::Surface(s) => s.alive(),
            FocusTarget::X11(x) => x.alive(),
        }
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            FocusTarget::Surface(s) => Some(Cow::Borrowed(s)),
            FocusTarget::X11(x) => x.wl_surface().map(Cow::Owned),
        }
    }
}

/// Forward a call to the target behind the focus.
macro_rules! forward {
    ($self:ident, $t:ident => $call:expr) => {
        match $self {
            FocusTarget::Surface($t) => $call,
            FocusTarget::X11($t) => $call,
        }
    };
}

impl KeyboardTarget<Lumen> for FocusTarget {
    fn enter(&self, seat: &Seat<Lumen>, data: &mut Lumen, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        forward!(self, t => KeyboardTarget::enter(t, seat, data, keys, serial))
    }

    fn leave(&self, seat: &Seat<Lumen>, data: &mut Lumen, serial: Serial) {
        forward!(self, t => KeyboardTarget::leave(t, seat, data, serial))
    }

    fn key(&self, seat: &Seat<Lumen>, data: &mut Lumen, key: KeysymHandle<'_>, state: KeyState, serial: Serial, time: u32) {
        forward!(self, t => KeyboardTarget::key(t, seat, data, key, state, serial, time))
    }

    fn modifiers(&self, seat: &Seat<Lumen>, data: &mut Lumen, modifiers: ModifiersState, serial: Serial) {
        forward!(self, t => KeyboardTarget::modifiers(t, seat, data, modifiers, serial))
    }
}

impl PointerTarget<Lumen> for FocusTarget {
    fn enter(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &MotionEvent) {
        forward!(self, t => PointerTarget::enter(t, seat, data, event))
    }

    fn motion(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &MotionEvent) {
        forward!(self, t => PointerTarget::motion(t, seat, data, event))
    }

    fn relative_motion(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &RelativeMotionEvent) {
        forward!(self, t => PointerTarget::relative_motion(t, seat, data, event))
    }

    fn button(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &ButtonEvent) {
        forward!(self, t => PointerTarget::button(t, seat, data, event))
    }

    fn axis(&self, seat: &Seat<Lumen>, data: &mut Lumen, frame: AxisFrame) {
        forward!(self, t => PointerTarget::axis(t, seat, data, frame))
    }

    fn frame(&self, seat: &Seat<Lumen>, data: &mut Lumen) {
        forward!(self, t => PointerTarget::frame(t, seat, data))
    }

    fn leave(&self, seat: &Seat<Lumen>, data: &mut Lumen, serial: Serial, time: u32) {
        forward!(self, t => PointerTarget::leave(t, seat, data, serial, time))
    }

    fn gesture_swipe_begin(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GestureSwipeBeginEvent) {
        forward!(self, t => PointerTarget::gesture_swipe_begin(t, seat, data, event))
    }

    fn gesture_swipe_update(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GestureSwipeUpdateEvent) {
        forward!(self, t => PointerTarget::gesture_swipe_update(t, seat, data, event))
    }

    fn gesture_swipe_end(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GestureSwipeEndEvent) {
        forward!(self, t => PointerTarget::gesture_swipe_end(t, seat, data, event))
    }

    fn gesture_pinch_begin(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GesturePinchBeginEvent) {
        forward!(self, t => PointerTarget::gesture_pinch_begin(t, seat, data, event))
    }

    fn gesture_pinch_update(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GesturePinchUpdateEvent) {
        forward!(self, t => PointerTarget::gesture_pinch_update(t, seat, data, event))
    }

    fn gesture_pinch_end(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GesturePinchEndEvent) {
        forward!(self, t => PointerTarget::gesture_pinch_end(t, seat, data, event))
    }

    fn gesture_hold_begin(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GestureHoldBeginEvent) {
        forward!(self, t => PointerTarget::gesture_hold_begin(t, seat, data, event))
    }

    fn gesture_hold_end(&self, seat: &Seat<Lumen>, data: &mut Lumen, event: &GestureHoldEndEvent) {
        forward!(self, t => PointerTarget::gesture_hold_end(t, seat, data, event))
    }
}
