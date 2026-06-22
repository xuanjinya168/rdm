//! 在 NTFS 上将预分配的 `.part` 文件标记为稀疏文件。
//!
//! 若不做此标记，一个靠后的分段首次写入时，文件系统必须
//! 把此前所有字节都清零，导致在多 GB 文件上让并行工作线程
//! 阻塞数秒。从 Python 的 `_mark_sparse` 辅助函数移植而来。

use std::fs::File;

/// 尽力为 `file` 请求稀疏存储：调用失败会被忽略 —— 非稀疏文件
/// 只是变慢，不会产生错误结果；在非 Windows 平台上该函数为空操作
/// （这些文件系统本身就采用惰性分配）。
#[cfg(windows)]
pub fn mark_sparse(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    // FSCTL_SET_SPARSE 控制码
    const FSCTL_SET_SPARSE: u32 = 0x000900C4;
    let handle = file.as_raw_handle();
    let mut returned: u32 = 0;
    // 安全性：`handle` 是由 `file` 拥有的有效文件句柄；所有缓冲区
    // 指针均为空且长度为零，DeviceIoControl 对此控制码是接受的，
    // 且 `returned` 是一个有效的输出指针。
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
