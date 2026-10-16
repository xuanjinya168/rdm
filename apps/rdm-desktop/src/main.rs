//! RDM 桌面应用。界面由 GPUI 绘制，下载、解析和浏览器桥都在本进程内。

mod events;
mod format;
mod forms;
mod session;
mod shell;
mod tasks;
mod ui;
mod widgets;

fn main() {
    ui::run();
}
