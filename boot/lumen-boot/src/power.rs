//! Reboot, power off and "reboot into firmware setup".

use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::{Status, cstr16};

const BOOT_TO_FW_UI: u64 = 1;

/// Whether the firmware advertises EFI_OS_INDICATIONS_BOOT_TO_FW_UI.
pub fn firmware_ui_supported() -> bool {
    let mut buf = [0u8; 8];
    match runtime::get_variable(cstr16!("OsIndicationsSupported"), &VariableVendor::GLOBAL_VARIABLE, &mut buf) {
        Ok((data, _)) if data.len() == 8 => u64::from_le_bytes(buf) & BOOT_TO_FW_UI != 0,
        _ => false,
    }
}

/// Ask the firmware to show its setup UI on the next boot and reboot.
pub fn reboot_to_firmware() -> Status {
    let mut buf = [0u8; 8];
    let current = match runtime::get_variable(cstr16!("OsIndications"), &VariableVendor::GLOBAL_VARIABLE, &mut buf) {
        Ok((data, _)) if data.len() == 8 => u64::from_le_bytes(buf),
        _ => 0,
    };
    let value = (current | BOOT_TO_FW_UI).to_le_bytes();
    let attrs = VariableAttributes::NON_VOLATILE
        | VariableAttributes::BOOTSERVICE_ACCESS
        | VariableAttributes::RUNTIME_ACCESS;
    match runtime::set_variable(cstr16!("OsIndications"), &VariableVendor::GLOBAL_VARIABLE, attrs, &value) {
        Ok(()) => runtime::reset(ResetType::COLD, Status::SUCCESS, None),
        Err(e) => e.status(),
    }
}

pub fn reboot() -> ! {
    runtime::reset(ResetType::COLD, Status::SUCCESS, None)
}

pub fn poweroff() -> ! {
    runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None)
}
