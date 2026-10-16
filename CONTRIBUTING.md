# Contributing

RDM 是一个 Rust workspace，加上独立 workspace 里的 GPUI 桌面应用
（`apps/rdm-desktop/`）和 MV3 浏览器扩展。

## 环境

- Rust stable（后端和桌面都用根目录 `rust-toolchain.toml`）
- Node ≥ 18（浏览器扩展，以及版本号一致性检查）
- Linux 上编译桌面端需要 Wayland/X11、Vulkan 和 FreeType 开发包

## 提交前必跑

后端:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

桌面:

```powershell
cd apps/rdm-desktop
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

准备发布时还需执行根目录的 [`SMOKE_TEST.md`](SMOKE_TEST.md)。

## 设计约束

- 各 crate 单向依赖:`domain ← storage ← http ← engine ← service ← rdm-desktop`。
- UI 只消费来自 `DownloadManager` 的不可变任务快照。
- 共享可变状态只在持有对应锁时访问;`std::sync::Mutex` 的 guard 绝不跨 `.await`。
- 下载 worker 在报告终态前先持久化分段进度。
- 文件发布(publish)绝不覆盖已存在的输出文件。
- 数据库变更需要有序迁移加一条 schema 版本测试,并保持与现有库兼容。
- 注释解释并发 / 持久化 / 协议不变量,而非复述语句。

源文件使用 UTF-8、LF、四空格缩进。
