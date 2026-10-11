//! Clipboard and primary selection globals.
//!
//! smithay offers the current selection to a client when it gains keyboard
//! focus, but not to a data device the client creates *after* that (common:
//! toolkits create it lazily, after mapping their first window). Such a client
//! would see an empty clipboard until focus changes. Here the device-manager
//! requests are dispatched through smithay and, after a device is created,
//! the selection is offered again to the focused client — what the protocol
//! intends ("sent to a client immediately before receiving keyboard focus").

use smithay::reexports::wayland_protocols::wp::primary_selection::zv1::server::{
    zwp_primary_selection_device_manager_v1::{self as primary_manager, ZwpPrimarySelectionDeviceManagerV1},
    zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
};
use smithay::reexports::wayland_server::protocol::wl_data_device::WlDataDevice;
use smithay::reexports::wayland_server::protocol::wl_data_device_manager::{self, WlDataDeviceManager};
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};
use smithay::wayland::selection::data_device::{set_data_device_focus, DataDeviceState, DataDeviceUserData, DataSourceUserData};
use smithay::wayland::selection::primary_selection::{
    set_primary_focus, PrimaryDeviceManagerGlobalData, PrimaryDeviceUserData, PrimarySelectionState, PrimarySourceUserData,
};

use crate::state::Lumen;

impl Lumen {
    /// Offer the selections again if `client` has keyboard focus.
    fn reoffer_selection(&mut self, dh: &DisplayHandle, client: &Client) {
        let focused = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let has_focus = focused.and_then(|s| dh.get_client(s.id()).ok()).is_some_and(|c| &c == client);
        if has_focus {
            let seat = self.seat.clone();
            set_data_device_focus(dh, &seat, None);
            set_data_device_focus(dh, &seat, Some(client.clone()));
            set_primary_focus(dh, &seat, None);
            set_primary_focus(dh, &seat, Some(client.clone()));
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Lumen {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        data: &(),
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let new_device = matches!(request, wl_data_device_manager::Request::GetDataDevice { .. });
        <DataDeviceState as Dispatch<WlDataDeviceManager, (), Lumen>>::request(state, client, resource, request, data, dh, data_init);
        if new_device {
            state.reoffer_selection(dh, client);
        }
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceManagerV1, ()> for Lumen {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ZwpPrimarySelectionDeviceManagerV1,
        request: primary_manager::Request,
        data: &(),
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let new_device = matches!(request, primary_manager::Request::GetDevice { .. });
        <PrimarySelectionState as Dispatch<ZwpPrimarySelectionDeviceManagerV1, (), Lumen>>::request(
            state, client, resource, request, data, dh, data_init,
        );
        if new_device {
            state.reoffer_selection(dh, client);
        }
    }
}

smithay::reexports::wayland_server::delegate_global_dispatch!(Lumen: [WlDataDeviceManager: ()] => DataDeviceState);
smithay::reexports::wayland_server::delegate_dispatch!(Lumen: [WlDataDevice: DataDeviceUserData] => DataDeviceState);
smithay::reexports::wayland_server::delegate_dispatch!(Lumen: [WlDataSource: DataSourceUserData] => DataDeviceState);
smithay::reexports::wayland_server::delegate_global_dispatch!(
    Lumen: [ZwpPrimarySelectionDeviceManagerV1: PrimaryDeviceManagerGlobalData] => PrimarySelectionState
);
smithay::reexports::wayland_server::delegate_dispatch!(Lumen: [ZwpPrimarySelectionDeviceV1: PrimaryDeviceUserData] => PrimarySelectionState);
smithay::reexports::wayland_server::delegate_dispatch!(Lumen: [ZwpPrimarySelectionSourceV1: PrimarySourceUserData] => PrimarySelectionState);
