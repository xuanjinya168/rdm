//! Mark a preallocated `.part` file as sparse on NTFS.
//!
//! Without this, the first write of a late segment forces the filesystem to
//! zero-fill everything before it, stalling parallel workers for seconds on
//! multi-gigabyte files. Port of the Python `_mark_sparse` helper.

use std::fs::File;

/// Best-effort: request sparse storage for `file`. Errors are ignored — a
/// non-sparse file is merely slower, not wrong — and on non-Windows platforms
/// this is a no-op (those filesystems already allocate lazily).
#[cfg(windows)]
pub fn mark_sparse(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    // FSCTL_SET_SPARSE
    const FSCTL_SET_SPARSE: u32 = 0x000900C4;
    let handle = file.as_raw_handle();
    let mut returned: u32 = 0;
    // SAFETY: `handle` is a live file handle owned by `file`; all buffer
    // pointers are null with zero lengths, which DeviceIoControl accepts for
    // this control code, and `returned` is a valid out-pointer.
    unsafe {
        DeviceIoControl(
            handle as _,
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(windows))]
pub fn mark_sparse(_file: &File) {}
