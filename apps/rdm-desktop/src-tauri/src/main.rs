// 在 Windows 的 release 构建中隐藏控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    rdm_desktop_lib::run();
}
