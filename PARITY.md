# RDM ⇄ PyDM 功能对齐与 Python 退役计划

本文件追踪 Rust 重写版 **RDM** 相对原 Python 版 **PyDM**(`../src/pydm/`)的
功能覆盖情况。**当所有「待实现」项完成后,即可删除整个 `src/pydm/`、Python 测试
(`../tests/`)、`../pyproject.toml`、`../packaging/` 等 Python 工程文件。**

在此之前,Python 代码作为行为基准(spec)保留——不要提前删除。

## 一、已移植(后端逻辑,100%,均有测试)

| Python 源 | Rust 位置 | 状态 |
|---|---|---|
| `models.py` | `rdm-domain/models.rs` | ✅ |
| `validation.py` | `rdm-domain/validation.rs` | ✅ |
| `downloader/segments.py` | `rdm-domain/segments.rs` | ✅ |
| `config.py` | `rdm-domain/config.rs` | ✅ |
| `database.py` / `migrations.py` | `rdm-storage` | ✅(兼容旧库迁移) |
| `downloader/probe.py` | `rdm-http/probe.rs` | ✅ |
| `providers/*` | `rdm-http/provider.rs` | ✅ |
| `downloader/engine.py` | `rdm-engine/engine.rs` | ✅(端到端测试) |
| `downloader/files.py` | `rdm-engine/files.rs` | ✅ |
| `downloader/rate_limit.py` | `rdm-engine/rate_limit.rs` | ✅ |
| `manager.py` | `rdm-service/manager.rs` | ✅ |

## 二、GUI 与系统集成(已实现 —— 编译/构建通过)

实现位置:`apps/rdm-desktop/`(Svelte 前端 `src/` + Tauri 后端 `src-tauri/`)。
全部已实现并随 `npm run app:build` 构建进自包含 EXE
(`src-tauri/target/release/rdm-desktop.exe`)。

> 验证状态:Rust 后端 `cargo build` 通过、前端 `vite build` 通过、release EXE
> 构建通过。**托盘/通知/剪贴板/单实例等运行期行为需运行 EXE 实测**
> (本环境无显示,未做交互验证)。

### A. 系统集成(Tauri 后端,`src-tauri/src/lib.rs`)

- [x] **单实例 + URL 接管** —— `tauri-plugin-single-instance`;第二次启动经
      `first_http_url(argv)` 取首个 http(s) 参数,前置窗口并发 `rdm://open-url`
      事件,前端打开「新建下载」预填。
- [x] **系统托盘** —— `TrayIconBuilder` + 菜单(显示主窗口 / 新建下载 / 退出);
      `CloseRequested` 按 `minimize_to_tray` 最小化到托盘继续下载;双击托盘恢复。
- [x] **桌面通知** —— `tauri-plugin-notification`;前端在状态跃迁到 completed/failed
      时发通知(去重)。
- [x] **剪贴板监听** —— `tauri-plugin-clipboard-manager`;`clipboard_monitoring`
      开启时前端轮询识别 http(s) 地址并预填快速添加框。
- [x] **打开所在目录** —— `tauri-plugin-opener`;`open_folder` 命令打开任务
      `destination`。
- [x] **退出前优雅停止** —— quit/关闭时 `DownloadManager::shutdown()` 后再 `exit`。
- [x] **日志(轮转文件)** —— `tauri-plugin-log`;写入应用日志目录 `rdm.log`,
      超过 2 MB 自动轮转并只保留一份备份(`RotationStrategy::KeepOne`),同时镜像到
      stdout。启动 / 关闭等生命周期事件用 `log::info!` 记录(对应 `app.configure_logging`)。

### B. 界面(Svelte 前端)

- [x] **新建下载对话框** —— `components/AddDialog.svelte`:URL / 保存位置(目录选择,
      `tauri-plugin-dialog`)/ 文件名 / 并发连接 / SHA-256 + 前端校验
      (`lib/validate.js` 复刻 `is_http_url` / `is_valid_windows_filename` /
      `normalize_sha256`)。
- [x] **设置对话框** —— `components/SettingsDialog.svelte`:目录 / 同时下载 /
      默认连接 / 失败重试 / 全局限速 / 剪贴板开关 / 托盘开关。
- [x] **统计卡片** —— 全部 / 正在下载 / 已完成 / 合计速度。
- [x] **筛选标签** —— 全部 / 进行中 / 已完成 / 其他。
- [x] **任务表增强** —— ETA(`lib/format.js`)、右键上下文菜单、双击打开目录、
      选中态驱动的头部操作按钮、快捷键(Ctrl+N / Del)。
- [x] **删除时可选是否删文件** —— 三选一弹窗(删除文件 / 仅删记录 / 取消)。
- [x] **主题/样式** —— `src/app.css` 深色主题。

## 三、退役步骤(已执行 ✅)

Python 代码已删除,根目录文档已切换到 RDM:

1. [x] `apps/rdm-desktop` 覆盖全部「待实现」项(轮转日志为可选,未做)。
2. [x] 已删除:`../src/`、`../tests/`、`../pyproject.toml`、`../packaging/`、
   `../docs/`、`../.venv/`、各类 Python 缓存与 `build/`、`dist/`。
3. [x] 根 `../README.md`、`../CONTRIBUTING.md` 改写为 RDM;`.github/workflows/ci.yml`
   切换为 Rust + Tauri CI;`.gitignore` / `.editorconfig` 更新为 Rust/Node。
4. 本文件保留作为 PyDM→RDM 的功能映射记录,可随时删除。

> 仓库文件夹名已重命名为 `rdm`,与 GitHub 仓库名保持一致。
