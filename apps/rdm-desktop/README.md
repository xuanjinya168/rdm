# RDM Desktop

基于 Tauri 2 + Svelte 5 的前端,运行于 RDM 后端 crate(`rdm-service` →
`rdm-engine` / `rdm-http` / `rdm-storage` / `rdm-domain`)之上。

## 运行

需要 Node ≥ 18。Rust 工具链由 `src-tauri/rust-toolchain.toml` 固定为
**1.88.0**（见下方「构建约束」），首次会自动下载。

```powershell
npm install
npm run app:dev
```

`app:dev` 会先启动 Vite（`http://localhost:5173`），再编译 `src-tauri`
并打开窗口。首次编译拉取 Tauri 依赖，耗时较长。

生成不依赖 Vite、可直接运行的可执行文件：

```powershell
npm run app:build
src-tauri\target\release\rdm-desktop.exe
```

需要安装包时运行：

```powershell
npm run app:bundle
```

> 不要直接运行 `cargo build` 生成的 `target\debug\rdm-desktop.exe`。
> Tauri 调试构建加载的是 `devUrl`，没有同时运行 Vite 时窗口会显示空白。
> `app:build` 会启用 `custom-protocol` 并把 `dist` 前端资源嵌入 EXE。

## 构建约束（重要）

此环境的默认工具链是非常新的 rustc 1.96，Tauri 2.11 的传递依赖
`time 0.3.47/0.3.48` 在其上会触发 E0119 一致性（coherence）错误。已通过以下方式
解决，**请勿 `cargo update`**（否则会把 `time` 升回去而再次失败）：

- `src-tauri/rust-toolchain.toml` 固定 **1.88.0**（≥1.85 满足 edition2024，且满足
  其它依赖 1.88 的 MSRV）
- 提交的 `src-tauri/Cargo.lock` 把 `time` 锁到 **0.3.36**（通过 `serde_with 3.9`、
  `plist 1.6` 放开 `time ≥ 0.3.47` 的约束）

后端 workspace（`../../..`）不受影响，仍用 stable 1.96。

> 打包（`tauri build`）已提供 `src-tauri/icons/icon.ico`/`icon.png` 占位图标；
> 如需多分辨率图标可用 `cargo tauri icon` 重新生成。

## 结构

```
apps/rdm-desktop/
├─ src/                Svelte UI（App.svelte / lib/api.js）
├─ index.html, vite.config.js, svelte.config.js, package.json
└─ src-tauri/          Tauri 后端（独立 workspace）
   ├─ src/lib.rs       #[tauri::command] 包装 DownloadManager
   ├─ tauri.conf.json  窗口与构建配置
   └─ capabilities/    前端权限
```

后端通过 `task://update` 事件向前端推送实时进度；命令见
`src-tauri/src/lib.rs`（list_tasks / add_download / start_task / pause_task /
cancel_task / delete_task / reveal_task_file / resolve_media / get_settings /
save_settings）。
