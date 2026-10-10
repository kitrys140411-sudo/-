//! Starting a Linux kernel through its EFI stub.
//!
//! The kernel image is loaded with LoadImage from memory and started with
//! StartImage. The command line goes into LoadedImage.LoadOptions as UCS-2.
//! The initramfs is served through EFI_LOAD_FILE2_PROTOCOL installed on a handle
//! whose device path is a single vendor media node with LINUX_EFI_INITRD_MEDIA_GUID
//! (the mechanism the EFI stub uses since Linux 5.8).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

use bootcfg::{cmdline_ucs2, esp_path};
use uefi::boot::{self, LoadImageSource, OpenProtocolAttributes, OpenProtocolParams};
use uefi::fs::{FileSystem, Path};
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::{CString16, Guid, Handle, Status, guid};
use uefi_raw::Boolean;
use uefi_raw::protocol::device_path::DevicePathProtocol;
use uefi_raw::protocol::media::LoadFile2Protocol;

const LINUX_EFI_INITRD_MEDIA_GUID: Guid = guid!("5568e427-68fc-4f3d-ac74-ca555231cc68");

/// A user-facing boot failure: a short title and an explanation.
pub struct BootError {
    pub title: String,
    pub detail: String,
}

fn fail(title: &str, detail: String) -> BootError {
    BootError {
        title: title.into(),
        detail,
    }
}

/// Read a whole file from the partition the boot loader was started from.
pub fn read_esp_file(path: &str) -> Result<Vec<u8>, String> {
    let path = esp_path(path);
    let wide = CString16::try_from(path.as_str()).map_err(|_| format!("invalid path {path}"))?;
    let sfs = boot::get_image_file_system(boot::image_handle())
        .map_err(|e| format!("boot partition is not readable ({:?})", e.status()))?;
    let mut fs = FileSystem::new(sfs);
    fs.read(Path::new(&wide)).map_err(|e| match e {
        uefi::fs::Error::Io(io) if io.uefi_error.status() == Status::NOT_FOUND => {
            format!("{path} does not exist on the boot partition")
        }
        other => format!("cannot read {path}: {other}"),
    })
}

#[repr(C, packed)]
struct VendorMediaPath {
    // Vendor-defined media device path node.
    ty: u8,
    sub_ty: u8,
    len: [u8; 2],
    guid: [u8; 16],
    // End of entire device path.
    end_ty: u8,
    end_sub_ty: u8,
    end_len: [u8; 2],
}

/// LoadFile2 implementation serving the initrd bytes. `proto` must stay first:
/// the firmware passes a pointer to it back as `this`.
#[repr(C)]
struct InitrdLoader {
    proto: LoadFile2Protocol,
    data: Vec<u8>,
}

unsafe extern "efiapi" fn initrd_load_file(
    this: *mut LoadFile2Protocol,
    file_path: *const DevicePathProtocol,
    boot_policy: Boolean,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    if this.is_null() || buffer_size.is_null() || file_path.is_null() {
        return Status::INVALID_PARAMETER;
    }
    if boot_policy == Boolean::TRUE {
        return Status::UNSUPPORTED;
    }
    // SAFETY: `this` is the first field of an InitrdLoader we installed.
    let loader = unsafe { &*(this as *const InitrdLoader) };
    let len = loader.data.len();
    // SAFETY: checked non-null above; the caller owns the storage.
    unsafe {
        if buffer.is_null() || *buffer_size < len {
            *buffer_size = len;
            return Status::BUFFER_TOO_SMALL;
        }
        core::ptr::copy_nonoverlapping(loader.data.as_ptr(), buffer.cast::<u8>(), len);
        *buffer_size = len;
    }
    Status::SUCCESS
}

/// The installed initrd handle; uninstalled again if the kernel returns.
struct InitrdHandle {
    handle: Handle,
    path: *mut VendorMediaPath,
    loader: *mut InitrdLoader,
}

impl InitrdHandle {
    fn install(data: Vec<u8>) -> Result<Self, BootError> {
        let g = LINUX_EFI_INITRD_MEDIA_GUID.to_bytes();
        let path = Box::into_raw(Box::new(VendorMediaPath {
            ty: 0x04,     // MEDIA_DEVICE_PATH
            sub_ty: 0x03, // MEDIA_VENDOR_DP
            len: 20u16.to_le_bytes(),
            guid: g,
            end_ty: 0x7f,
            end_sub_ty: 0xff,
            end_len: 4u16.to_le_bytes(),
        }));
        let loader = Box::into_raw(Box::new(InitrdLoader {
            proto: LoadFile2Protocol {
                load_file: initrd_load_file,
            },
            data,
        }));
        // SAFETY: both interfaces are heap allocations that outlive the handle.
        let handle = unsafe {
            boot::install_protocol_interface(None, &DevicePathProtocol::GUID, path.cast())
        }
        .map_err(|e| fail("Cannot provide the initramfs", format!("InstallProtocolInterface(DevicePath) failed: {:?}", e.status())))?;
        // SAFETY: as above.
        if let Err(e) = unsafe {
            boot::install_protocol_interface(Some(handle), &LoadFile2Protocol::GUID, loader.cast())
        } {
            // SAFETY: undo the first install; the memory is still ours.
            unsafe {
                let _ = boot::uninstall_protocol_interface(handle, &DevicePathProtocol::GUID, path.cast());
                drop(Box::from_raw(path));
                drop(Box::from_raw(loader));
            }
            return Err(fail(
                "Cannot provide the initramfs",
                format!("InstallProtocolInterface(LoadFile2) failed: {:?} (is another initrd provider active?)", e.status()),
            ));
        }
        Ok(InitrdHandle { handle, path, loader })
    }
}

impl Drop for InitrdHandle {
    fn drop(&mut self) {
        // SAFETY: we installed exactly these interfaces on this handle.
        unsafe {
            let _ = boot::uninstall_protocol_interface(self.handle, &LoadFile2Protocol::GUID, self.loader.cast());
            let _ = boot::uninstall_protocol_interface(self.handle, &DevicePathProtocol::GUID, self.path.cast());
            drop(Box::from_raw(self.loader));
            drop(Box::from_raw(self.path));
        }
    }
}

/// Check that a buffer looks like a PE/COFF EFI application before LoadImage,
/// so a wrong file produces a clear message instead of a bare LOAD_ERROR.
fn check_pe(data: &[u8], path: &str) -> Result<(), BootError> {
    let bad = || {
        fail(
            "Kernel is not an EFI image",
            format!("{path} is not a PE/COFF executable; build the kernel with CONFIG_EFI_STUB=y"),
        )
    };
    if data.len() < 0x40 || &data[..2] != b"MZ" {
        return Err(bad());
    }
    let pe = u32::from_le_bytes([data[0x3c], data[0x3d], data[0x3e], data[0x3f]]) as usize;
    if data.len() < pe + 6 || &data[pe..pe + 4] != b"PE\0\0" {
        return Err(bad());
    }
    let machine = u16::from_le_bytes([data[pe + 4], data[pe + 5]]);
    if machine != 0x8664 {
        return Err(fail(
            "Kernel has the wrong architecture",
            format!("{path} targets machine type {machine:#06x}, expected x86_64 (0x8664)"),
        ));
    }
    Ok(())
}

/// Full device path of a file on the boot loader's own partition
/// (partition device path + FilePath node). EDK2 rejects LoadImage from a
/// buffer without a device path, and the kernel may inspect its FilePath.
fn esp_file_device_path<'a>(path: &str, storage: &'a mut Vec<u8>) -> Option<&'a DevicePath> {
    let params = |handle| OpenProtocolParams { handle, agent: boot::image_handle(), controller: None };
    // SAFETY: GetProtocol borrows the interfaces without taking ownership.
    let li = unsafe { boot::open_protocol::<LoadedImage>(params(boot::image_handle()), OpenProtocolAttributes::GetProtocol) }.ok()?;
    let device = li.device()?;
    // SAFETY: as above.
    let dp = unsafe { boot::open_protocol::<DevicePath>(params(device), OpenProtocolAttributes::GetProtocol) }.ok()?;
    let wide = CString16::try_from(esp_path(path).as_str()).ok()?;
    let mut builder = DevicePathBuilder::with_vec(storage);
    for node in dp.node_iter() {
        builder = builder.push(&node).ok()?;
    }
    builder.push(&build::media::FilePath { path_name: &wide }).ok()?.finalize().ok()
}

pub struct Prepared {
    kernel: Handle,
    _initrd: Option<InitrdHandle>,
    _options: Vec<u16>,
}

/// Load everything needed to start the kernel; nothing irreversible happens yet.
pub fn prepare(kernel: &str, initrd: Option<&str>, cmdline: &str) -> Result<Prepared, BootError> {
    let image = read_esp_file(kernel).map_err(|e| fail("Kernel not found", e))?;
    check_pe(&image, kernel)?;
    let initrd = match initrd {
        Some(p) => {
            let data = read_esp_file(p).map_err(|e| fail("Initramfs not found", e))?;
            Some(InitrdHandle::install(data)?)
        }
        None => None,
    };
    let mut dp_storage = Vec::new();
    let file_path = esp_file_device_path(kernel, &mut dp_storage);
    let handle = boot::load_image(boot::image_handle(), LoadImageSource::FromBuffer { buffer: &image, file_path })
    .map_err(|e| {
        let why = match e.status() {
            Status::SECURITY_VIOLATION | Status::ACCESS_DENIED => {
                "the firmware rejected it (Secure Boot is enabled; Lumen boots with Secure Boot off)"
            }
            Status::UNSUPPORTED => "the firmware does not support this image type",
            Status::OUT_OF_RESOURCES => "not enough memory",
            _ => "the image is damaged or not an EFI application",
        };
        fail("Cannot load the kernel", format!("LoadImage({kernel}) failed with {:?}: {why}", e.status()))
    })?;
    let options = cmdline_ucs2(cmdline);
    {
        let mut li = boot::open_protocol_exclusive::<LoadedImage>(handle).map_err(|e| {
            let _ = boot::unload_image(handle);
            fail("Cannot load the kernel", format!("LoadedImage protocol missing: {:?}", e.status()))
        })?;
        // SAFETY: `options` is kept alive in Prepared until StartImage returns.
        unsafe { li.set_load_options(options.as_ptr().cast(), (options.len() * 2) as u32) };
    }
    Ok(Prepared {
        kernel: handle,
        _initrd: initrd,
        _options: options,
    })
}

/// Start the kernel. Only returns if the kernel failed to start or exited.
pub fn start(p: Prepared) -> BootError {
    let r = boot::start_image(p.kernel);
    let _ = boot::unload_image(p.kernel);
    match r {
        Ok(()) => fail("The kernel exited", "The kernel returned to the boot loader without starting.".into()),
        Err(e) => fail(
            "The kernel failed to start",
            format!("StartImage returned {:?}. Check the kernel command line and that the initramfs matches the kernel.", e.status()),
        ),
    }
}
