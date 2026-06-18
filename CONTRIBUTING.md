# Contributing

RDM 是一个 Rust workspace 加一个 Tauri + Svelte 桌面应用
(`apps/rdm-desktop/`)。

## 环境

- Rust stable(后端 workspace,见 `rust-toolchain.toml`)
- Node ≥ 18(桌面前端)
- 桌面应用的 Rust 端固定 1.88.0(见 `apps/rdm-desktop/src-tauri/rust-toolchain.toml`
  及该处「构建约束」注释——不要在 `src-tauri` 下 `cargo update`)

## 提交前必跑

后端:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

桌面前端:

```powershell
cd apps/rdm-desktop
npm run check
npm test
npm run build
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
