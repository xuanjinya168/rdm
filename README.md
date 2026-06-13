# RDM

RDM 是一个 Windows HTTP/HTTPS 多连接分段下载管理器,使用 **Rust** 编写,
桌面界面基于 **Tauri 2 + Svelte 5**。它是早期 Python/PySide6 版本(PyDM)的
完整重写。

## 功能

- HTTP/HTTPS 多连接分段下载,动态分段窃取
- 暂停、继续、取消和失败重试
- SQLite 任务持久化,程序重启后断点续传
- 可选 SHA-256 完整性校验
- 下载队列、并发任务数量与全局限速
- 不支持 Range 的服务器自动降级为单连接
- 剪贴板 URL 识别、系统托盘、下载完成通知、单实例 URL 接管
- `.part` 临时文件、文件大小校验及原子重命名
- 可注册下载 Provider,为认证 / 签名链接预留扩展点

## 项目结构

```
rdm/
├─ crates/
│  ├─ rdm-domain     领域模型、校验、分段规划、配置(纯逻辑)
│  ├─ rdm-storage    SQLite 持久化 + 迁移
│  ├─ rdm-http       reqwest 客户端、URL 探测、Provider
│  ├─ rdm-engine     分段下载引擎(tokio):动态分段、续传、限速、SHA-256
│  └─ rdm-service    下载队列 / 调度器(DownloadManager)
└─ apps/
   └─ rdm-desktop    Svelte 前端(src/)+ Tauri 后端(src-tauri/)
```

依赖方向:`domain ← storage ← http ← engine ← service ← rdm-desktop`。

## 构建与测试

### 后端(Rust workspace)

需要 Rust stable(见 `rdm/rust-toolchain.toml`)。

```powershell
cd rdm
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### 桌面应用

需要 Node ≥ 18。Tauri 端的工具链由 `rdm/apps/rdm-desktop/src-tauri/rust-toolchain.toml`
固定(详见该目录 README 的「构建约束」)。

```powershell
cd rdm/apps/rdm-desktop
npm install
npm run app:dev      # 开发(热重载)
npm run app:build    # 生成自包含 EXE: src-tauri/target/release/rdm-desktop.exe
```

详见 [`rdm/README.md`](rdm/README.md) 与
[`rdm/apps/rdm-desktop/README.md`](rdm/apps/rdm-desktop/README.md)。

## 数据目录

配置、SQLite 数据库默认保存在 `%LOCALAPPDATA%\RDM`。数据库与旧版 PyDM 兼容
(沿用 `PRAGMA user_version` 迁移)。

首版不含浏览器扩展、视频嗅探、BT、FTP 或 DRM 下载。
