use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child, protocol::wl_output::WlOutput,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{Event as HandleEvent, State, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, Event as ManagerEvent, ZwlrForeignToplevelManagerV1},
};

use super::Desktop;

#[derive(Default)]
pub(super) struct Toplevel {
    outputs: Vec<WlOutput>,
    fullscreen: bool,
    activated: bool,
    covered: Vec<WlOutput>,
    active_cover: bool,
}

impl Toplevel {
    pub(super) fn covers(&self, output: &WlOutput) -> bool {
        self.covered.contains(output)
    }

    pub(super) fn actively_covers(&self, output: &WlOutput) -> bool {
        self.active_cover && self.covers(output)
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Desktop {
    fn event(
        desktop: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: ManagerEvent,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ManagerEvent::Toplevel { toplevel } => {
                desktop.toplevels.insert(toplevel.id(), Toplevel::default());
            }
            ManagerEvent::Finished => desktop.toplevels.clear(),
            _ => {}
        }
    }

    event_created_child!(Desktop, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Desktop {
    fn event(
        desktop: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: HandleEvent,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, HandleEvent::Closed) {
            desktop.toplevels.remove(&handle.id());
            handle.destroy();
            return;
        }
        let Some(toplevel) = desktop.toplevels.get_mut(&handle.id()) else {
            return;
        };
        match event {
            HandleEvent::OutputEnter { output } if !toplevel.outputs.contains(&output) => {
                toplevel.outputs.push(output);
            }
            HandleEvent::OutputLeave { output } => {
                toplevel.outputs.retain(|current| *current != output);
            }
            HandleEvent::State { state } => {
                let states = state
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .copied()
                    .map(u32::from_ne_bytes);
                let contains = |state: State| states.clone().any(|value| value == state as u32);
                // Output enter/leave describe visibility independently of keyboard focus.
                toplevel.fullscreen = contains(State::Fullscreen) && !contains(State::Minimized);
                toplevel.activated = contains(State::Activated);
            }
            HandleEvent::Done => {
                toplevel.active_cover = toplevel.activated && toplevel.fullscreen;
                if toplevel.fullscreen {
                    toplevel.covered.clone_from(&toplevel.outputs);
                } else {
                    toplevel.covered.clear();
                }
            }
            _ => {}
        }
    }
}
