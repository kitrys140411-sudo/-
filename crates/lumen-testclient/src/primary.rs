//! `--primary`: `--copy` and `--paste` use the primary selection
//! (zwp_primary_selection_v1: select to copy, middle click to paste) instead
//! of the clipboard. The output lines are the same (`copy event=..`,
//! `paste text=..`).

use crate::sctk;
use sctk::data_device_manager::{ReadPipe, WritePipe};
use sctk::delegate_primary_selection;
use sctk::primary_selection::PrimarySelectionManagerState;
use sctk::primary_selection::device::{PrimarySelectionDevice, PrimarySelectionDeviceHandler};
use sctk::primary_selection::selection::{PrimarySelectionSource, PrimarySelectionSourceHandler};
use sctk::reexports::client::globals::GlobalList;
use sctk::reexports::client::protocol::wl_seat;
use sctk::reexports::client::{Connection, QueueHandle};
use sctk::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1;
use sctk::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1;

use crate::App;
use crate::out::{self, emit};

pub struct Primary {
    manager: PrimarySelectionManagerState,
    device: Option<PrimarySelectionDevice>,
    source: Option<PrimarySelectionSource>,
}

impl Primary {
    pub fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<Primary> {
        match PrimarySelectionManagerState::bind(globals, qh) {
            Ok(manager) => Some(Primary { manager, device: None, source: None }),
            Err(e) => {
                eprintln!("lumen-testclient: zwp_primary_selection_device_manager_v1 not available: {e}");
                None
            }
        }
    }
}

impl App {
    pub(crate) fn primary_seat_ready(&mut self, seat: &wl_seat::WlSeat) {
        let qh = self.qh.clone();
        if let Some(p) = self.primary.as_mut() {
            p.device = Some(p.manager.get_selection_device(&qh, seat));
        }
    }

    /// `--copy --primary`: our text becomes the primary selection.
    pub(crate) fn offer_primary(&mut self, serial: u32) {
        let Some(p) = self.primary.as_mut() else { return };
        let Some(device) = p.device.as_ref() else { return };
        if p.source.is_some() {
            return;
        }
        let source = p.manager.create_selection_source(&self.qh, out::TEXT_MIME_TYPES);
        source.set_selection(device, serial);
        p.source = Some(source);
        emit("copy event=offered");
    }

    /// `--paste --primary`: a pipe with the primary selection as text.
    pub(crate) fn receive_primary(&self) -> Result<ReadPipe, &'static str> {
        let device = self.primary.as_ref().and_then(|p| p.device.as_ref()).ok_or("no-data-device")?;
        let offer = device.data().selection_offer().ok_or("no-selection")?;
        let mime = offer.with_mime_types(out::pick_text_mime).ok_or("no-text-mime")?;
        offer.receive(mime).map_err(|_| "receive-failed")
    }
}

impl PrimarySelectionDeviceHandler for App {
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpPrimarySelectionDeviceV1) {}
}

impl PrimarySelectionSourceHandler for App {
    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpPrimarySelectionSourceV1, mime: String, pipe: WritePipe) {
        self.send_selection(mime, pipe);
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &ZwpPrimarySelectionSourceV1) {
        if let Some(p) = self.primary.as_mut() {
            if p.source.as_ref().is_some_and(|s| s.inner() == source) {
                p.source = None;
                emit("copy event=cancelled");
            }
        }
    }
}

delegate_primary_selection!(App);
