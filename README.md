# RDM

RDM 是一个 Windows HTTP/HTTPS 多连接分段下载管理器，使用 **Rust** 编写。
桌面界面是原生窗口，基于 **Open GPUI**。

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
- X / Twitter、Instagram、Threads 帖子媒体解析
- 浏览器扩展（MV3）：支持下载拦截、媒体嗅探（图片/视频/音频/m3u8），右键菜单，
  详见 [`apps/rdm-extension/README.md`](apps/rdm-extension/README.md)

## 项目结构

```
├─ crates/
│  ├─ rdm-domain     领域模型、校验、分段规划、配置(纯逻辑)
│  ├─ rdm-storage    SQLite 持久化 + 迁移
│  ├─ rdm-http       reqwest 客户端、URL 探测、Provider
│  ├─ rdm-engine     分段下载引擎(tokio):动态分段、续传、限速、SHA-256
│  ├─ rdm-service    下载队列 / 调度器(DownloadManager)
│  ├─ rdm-resolver   X / Twitter、Instagram、Threads 媒体解析
│  └─ rdm-bridge     浏览器扩展使用的本地 HTTP 桥
└─ apps/
   ├─ rdm-desktop    GPUI 桌面窗口（独立 workspace，直接调用下载引擎）
   └─ rdm-extension  浏览器扩展(MV3):拦截下载交给桌面端
```

依赖方向：`domain ← storage ← http ← engine ← service ← rdm-desktop`，
`rdm-bridge` 只依赖 `rdm-domain`。桌面进程在界面线程之外启动 tokio，
再把进度和桥命令送进 GPUI。

## 构建与测试

### 后端(Rust workspace)

需要 Rust stable(见 `rust-toolchain.toml`)。

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### 桌面应用

需要 Rust stable。桌面 crate 不在根 workspace 里，避免后端测试编译 GPUI。

```powershell
cd apps/rdm-desktop
cargo test
cargo build --release   # target/release/rdm-desktop.exe
```

详见 [`apps/rdm-desktop/README.md`](apps/rdm-desktop/README.md)。

发布前还需按 [`SMOKE_TEST.md`](SMOKE_TEST.md) 在真实 Windows 桌面完成运行期验收。

### 浏览器扩展

需要 Node ≥ 18。扩展无打包步骤，以源码加载。

```powershell
cd apps/rdm-extension
npm test            # 运行纯逻辑单测（node --test）
npm run icons       # 重新生成占位图标（可选）
```

在 Chrome/Edge 中通过「加载已解压的扩展程序」选择 `apps/rdm-extension/`
目录即可安装，详见 [`apps/rdm-extension/README.md`](apps/rdm-extension/README.md)。

## 数据目录

配置、SQLite 数据库默认保存在 `%LOCALAPPDATA%\RDM`,迁移通过 SQLite
`PRAGMA user_version` 管理。

当前版本不含 BT、FTP 或 DRM 下载。浏览器扩展通过仅监听 127.0.0.1 的
本地 HTTP 桥与桌面端协作。
