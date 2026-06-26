# RDM 浏览器扩展（实验性）

一个 Manifest V3 浏览器扩展，把浏览器下载交给 **RDM 桌面端** 处理。
支持自动拦截下载，或通过右键菜单手动交给 RDM。

> 实验性功能。需要 RDM 桌面端（≥ 0.2.2）同时在运行。

## 工作原理

扩展与桌面端之间通过一个**仅监听 127.0.0.1** 的本地 HTTP 桥通信：

```
浏览器  ──(MV3 扩展拦截下载)──▶  POST http://127.0.0.1:43721/downloads  ──▶  RDM 桌面端
                                                                        （弹出确认框）
```

- 桌面端在启动时（Tauri setup hook）用 axum 起这个桥，绑定 `127.0.0.1:43721`。
- 扩展拦截到下载后，向 `POST /downloads` 提交 `{ url, filename? }`。
- 桥返回 **202 Accepted**，并通过 `rdm://external-download` 事件让桌面端弹出与「新建下载」
  相同的确认框（文件名/保存位置/连接数），用户点「开始下载」后才开始下载——
  **不会静默抢走下载**，体验对齐 IDM 的拦截确认。
- `GET /health` 返回 `{"status":"ok","version":"…"}`，扩展用它检测桌面端是否在线。

桥固定端口 `43721`，与 `apps/rdm-desktop/src-tauri/src/bridge.rs` 中的 `BRIDGE_PORT` 一致。
v1 不做可配置端口。

> **为什么不用 IDM 那种 native messaging？** Native messaging 需要写注册表 +
> 注册 native-host manifest，每换一个浏览器/机器都要重装，难以分发和测试。
> 本地 HTTP 桥零安装、跨 Chrome/Edge/Firefox、可直接单测，是更轻量的选择。

## 安装（加载已解压的扩展）

1. 确认 RDM 桌面端正在运行（桥随之监听 127.0.0.1:43721）。
2. 在 Chrome / Edge / Brave 等 Chromium 浏览器打开扩展管理页：
   - Chrome：`chrome://extensions`
   - Edge：`edge://extensions`
3. 打开右上角「开发者模式」。
4. 点「加载已解压的扩展程序」，选择本目录 `apps/rdm-extension/`。
5. 点击工具栏的 RDM 图标，可：
   - 开关「自动拦截下载」（**默认关闭**，避免一装就抢走所有下载）。
   - 点「测试连接」确认桌面端在线。

> Firefox：需把 `manifest.json` 的 `background.service_worker` 改为
> `scripts` 数组形式，或使用 [`browser_specific_settings`](https://developer.mozilla.org/docs/Mozilla/Add-ons/WebExtensions/manifest.json/browser_specific_settings)
> 并通过 `about:debugging` 临时加载。当前仓库仅针对 Chromium 适配。

## 使用方式

- **自动拦截（开关开启时）**：浏览器发起的下载若满足条件（http/https 直链、
  非网页/字体/脚本资源、体积 ≥ 100KB），会被取消浏览器侧下载，桌面端弹出
  确认框，用户点「开始下载」后才真正开始。**安全机制**：扩展会先确认桥已接收
  再取消浏览器下载，因此桌面端未运行时浏览器下载照常进行、不会丢失。
- **右键菜单（始终可用）**：在链接、图片/视频/音频或页面上右键 →「用 RDM 下载」，
  桌面端弹出确认框（非拦截式，不取消浏览器行为）。
- 桌面端未运行时，扩展会给出一次「RDM 未运行」通知，且不会刷屏（冷却 60s）。

## 开发与测试

无打包步骤，扩展以源码加载。纯逻辑（下载是否应被拦截）放在 `src/lib/intercept.js`，
用 `node --test` 单测，不依赖任何 `chrome.*` API：

```powershell
cd apps/rdm-extension
npm test            # node --test
npm run icons       # 重新生成占位 PNG 图标（无外部图像依赖）
```

### 目录结构

```
├─ manifest.json          MV3 清单
├─ src/
│  ├─ background.js       service worker：拦截 + 右键菜单 + 通知
│  ├─ popup.{html,css,js} 工具栏弹窗：开关 / 连接测试 / 状态
│  └─ lib/
│     ├─ config.js        桥地址、storage 键、阈值常量
│     ├─ bridge.js        postDownload / pingHealth（薄 fetch 封装）
│     ├─ intercept.js     shouldIntercept（纯函数，可单测）
│     └─ intercept.test.js
├─ icons/                 占位品牌色图标（16/48/128）
└─ scripts/make-icons.mjs 重生成图标
```

## 安全说明

- 桥仅绑定到 `127.0.0.1`，不对外网或局域网开放。
- 扩展只请求必要的权限：`downloads`（拦截）、`storage`（开关）、
  `contextMenus`（右键）、`notifications`（离线提示）。
- `host_permissions` 仅限 `http://127.0.0.1:43721/`，扩展不会读取任意网站内容。
