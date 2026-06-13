# RDM

RDM 是 [PyDM Desktop](../README.md) 的 Rust 重写版本：一个 Windows
HTTP/HTTPS 多连接分段下载管理器。原生、低内存。

## 项目结构

```
crates/
├─ rdm-domain     领域模型、校验、分段规划、配置（纯逻辑，无 I/O）
├─ rdm-storage    SQLite 持久化（rusqlite）+ user_version 迁移
├─ rdm-http       reqwest 客户端、URL 探测、Provider 扩展点
├─ rdm-engine     分段下载引擎（tokio）：动态分段/窃取、续传、限速、SHA-256
└─ rdm-service    编排层：下载队列/manager、剪贴板监听、单实例 IPC

apps/
└─ rdm-desktop
   ├─ src/         Svelte UI
   └─ src-tauri/   Tauri 后端（装配 rdm-service）
```

依赖方向：`domain ← storage ← http ← engine ← service ← rdm-desktop`。

## 技术栈

| 领域 | 选型 |
|---|---|
| 异步运行时 | tokio |
| HTTP 客户端 | reqwest（rustls 后端） |
| 限速 | 自定义令牌桶 / governor |
| 持久化 | rusqlite（bundled SQLite） |
| 序列化 | serde / serde_json |
| 错误处理 | thiserror（库层）/ anyhow（应用层） |
| GUI | Tauri + Svelte |
| 系统集成 | Tauri 托盘/通知、arboard 剪贴板 |

## 当前进度

- ✅ `rdm-domain` —— `DownloadTask` / `Segment` / `TaskStatus`、URL/SHA-256/文件名校验、分段规划与续传校验、`AppSettings` 与原子写入的 `SettingsStore`
- ✅ `rdm-storage` —— `DownloadDatabase`（WAL、单连接互斥）+ 与现有 PyDM 库兼容的迁移
- ✅ `rdm-http` —— 共享 reqwest 客户端、`probe_url`（size/range/etag/文件名）、`DownloadProvider` 注册表
- ✅ `rdm-engine` —— 异步分段下载引擎：动态分段窃取、NTFS 稀疏预分配、续传、限速、SHA-256，端到端测试
- ✅ `rdm-service` —— `DownloadManager` 队列编排与调度器
- ✅ `apps/rdm-desktop` —— Tauri 2 + Svelte 5 界面（已搭好，见其 README 运行）

后端 5 个 crate 共 50 个测试全绿。应用数据目录：`%LOCALAPPDATA%\RDM`。

## 构建与测试

需要 Rust stable 工具链（见 `rust-toolchain.toml`）。后端：

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

桌面端（独立 workspace，不随 `cargo test` 编译）见
[`apps/rdm-desktop/README.md`](apps/rdm-desktop/README.md)。
