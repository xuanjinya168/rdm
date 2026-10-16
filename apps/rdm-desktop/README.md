# RDM 桌面应用

原生 Rust 窗口，界面用 [Open GPUI](https://docs.rs/open-gpui) 0.2 绘制。下载队列、媒体解析和浏览器桥都在同一个进程里，不再经过 WebView。

## 构建

需要 Rust stable（见仓库根目录 `rust-toolchain.toml`）。Linux 上还需要 Wayland/X11、Vulkan 和 FreeType 的开发包。

```powershell
cd apps/rdm-desktop
cargo run
cargo test
cargo clippy --all-targets -- -D warnings
```

Release 可执行文件：

```powershell
cargo build --release
# apps/rdm-desktop/target/release/rdm-desktop.exe
```

这个 crate 自带空的 `[workspace]`，不加入根 workspace，这样后端 `cargo test` 不用编译 GPUI。它通过 path 依赖 `crates/` 里的领域层和下载引擎。

## 运行

```text
rdm-desktop.exe
rdm-desktop.exe https://example.com/file.zip
```

第二个进程拿不到实例锁时，会把地址交给已经在跑的实例（`POST /downloads`），没有地址时只把窗口带到前台（`POST /activate`）。

HLS 转码使用可执行文件旁边的 `ffmpeg` / `ffmpeg.exe`；找不到时再走 `PATH`。

## 快捷键

| 按键 | 作用 |
|---|---|
| Ctrl+N | 新建下载 |
| Ctrl+1 | 下载列表 |
| Ctrl+2 | 媒体解析 |
| Ctrl+Q | 退出 |
| Esc | 关闭最上层对话框 |
| Delete | 删除选中任务（输入框聚焦时不触发） |

## 本地桥

桌面进程在 `127.0.0.1:43721` 上提供与浏览器扩展兼容的 HTTP 桥，实现在 `crates/rdm-bridge`：

- `GET /health`
- `POST /downloads`（202，预填新建下载）
- `POST /media-candidates`（202，打开嗅探确认框）
- `POST /activate`（204，显示主窗口）
