//! 窗口之外的桌面集成：单实例、托盘、目录选择、文件定位、通知和剪贴板。

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

use rdm_domain::config::app_data_dir;
use rdm_domain::DownloadTask;

use crate::events::AppEvent;

/// 持有排他锁的进程继续启动。锁由进程退出时随文件句柄释放。
/// 打不开锁文件时不阻止启动，避免数据目录异常把程序卡死。
pub fn acquire_single_instance() -> bool {
    let path = app_data_dir().join("instance.lock");
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            log::warn!("无法创建数据目录，跳过单实例锁：{error}");
            return true;
        }
    }
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            log::warn!("无法打开实例锁，继续启动：{error}");
            return true;
        }
    };
    match file.try_lock() {
        Ok(()) => {
            Box::leak(Box::new(file));
            true
        }
        Err(_) => false,
    }
}

/// 把启动参数交给已经在跑的实例。桥还没绑定时会重试几秒。
pub fn handoff_to_running(url: Option<&str>) -> bool {
    let (path, body) = match url {
        Some(url) => ("/downloads", serde_json::json!({ "url": url }).to_string()),
        None => ("/activate", "{}".to_string()),
    };
    for _ in 0..25 {
        if post_local(path, &body) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn post_local(path: &str, body: &str) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], rdm_bridge::BRIDGE_PORT));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(400)) {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buffer = [0u8; 80];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let head = String::from_utf8_lossy(&buffer[..read]);
    head.contains(" 202 ") || head.contains(" 204 ") || head.contains(" 200 ")
}

pub fn notify(title: &str, body: &str) {
    if let Err(error) = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show()
    {
        log::debug!("通知未显示：{error}");
    }
}

pub fn pick_folder(current: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    if !current.trim().is_empty() {
        dialog = dialog.set_directory(current);
    }
    dialog
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned())
}

pub fn reveal_task(task: &DownloadTask) {
    let output = task.output_path();
    if output.is_file() {
        reveal_path(&output);
        return;
    }
    let directory = std::path::PathBuf::from(&task.destination);
    let _ = std::fs::create_dir_all(&directory);
    reveal_path(&directory);
}

fn reveal_path(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let directory = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = std::process::Command::new("xdg-open")
            .arg(directory)
            .spawn();
    }
}

/// 安装系统托盘。失败只记日志，主窗口仍然可用。
/// 图标对象泄漏到进程结束，避免托盘在函数返回时消失。
pub fn install_tray(tx: Sender<AppEvent>) {
    let menu = tray_icon::menu::Menu::new();
    let show = tray_icon::menu::MenuItem::with_id("show", "显示主窗口", true, None);
    let new_item = tray_icon::menu::MenuItem::with_id("new", "新建下载", true, None);
    let separator = tray_icon::menu::PredefinedMenuItem::separator();
    let quit = tray_icon::menu::MenuItem::with_id("quit", "退出", true, None);
    if let Err(error) = menu.append_items(&[&show, &new_item, &separator, &quit]) {
        log::warn!("托盘菜单创建失败：{error}");
        return;
    }
    let icon = match tray_icon() {
        Ok(icon) => icon,
        Err(error) => {
            log::warn!("托盘图标创建失败：{error}");
            return;
        }
    };
    let tray = match tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("RDM 下载管理器")
        .with_icon(icon)
        .build()
    {
        Ok(tray) => tray,
        Err(error) => {
            log::warn!("托盘创建失败：{error}");
            return;
        }
    };
    Box::leak(Box::new(tray));

    std::thread::spawn(move || loop {
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            let command = match event.id.as_ref() {
                "show" => Some(AppEvent::Bridge(rdm_bridge::BridgeCommand::Activate)),
                "new" => Some(AppEvent::NewDownload),
                "quit" => Some(AppEvent::Quit),
                _ => None,
            };
            if let Some(command) = command {
                if tx.send(command).is_err() {
                    return;
                }
            }
        }
        if let Ok(tray_icon::TrayIconEvent::Click {
            button: tray_icon::MouseButton::Left,
            button_state: tray_icon::MouseButtonState::Up,
            ..
        }) = tray_icon::TrayIconEvent::receiver().try_recv()
        {
            if tx
                .send(AppEvent::Bridge(rdm_bridge::BridgeCommand::Activate))
                .is_err()
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    });
}

fn tray_icon() -> Result<tray_icon::Icon, tray_icon::BadIcon> {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let edge = x < 2 || y < 2 || x + 2 >= SIZE || y + 2 >= SIZE;
            let mark = (12..20).contains(&x) && (8..24).contains(&y);
            if edge || mark {
                rgba.extend_from_slice(&[0x5b, 0x74, 0xff, 0xff]);
            } else {
                rgba.extend_from_slice(&[0x17, 0x1a, 0x23, 0xff]);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE)
}
