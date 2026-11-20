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
    focused_fullscreen: bool,
    covered: Vec<WlOutput>,
}

impl Toplevel {
    pub(super) fn covers(&self, output: &WlOutput) -> bool {
        self.covered.contains(output)
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
                // The protocol has no workspace visibility. Focus avoids pausing for hidden apps.
                toplevel.focused_fullscreen = contains(State::Fullscreen)
                    && contains(State::Activated)
                    && !contains(State::Minimized);
            }
            HandleEvent::Done if toplevel.focused_fullscreen => {
                toplevel.covered.clone_from(&toplevel.outputs);
            }
            HandleEvent::Done => toplevel.covered.clear(),
            _ => {}
        }
    }
}
