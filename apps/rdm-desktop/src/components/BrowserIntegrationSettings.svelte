<script>
  import AppIcon from "./AppIcon.svelte";
</script>

<div class="browser-settings">
  <section class="intro">
    <div class="intro-icon"><AppIcon name="browser" size={20} /></div>
    <div>
      <strong>浏览器集成</strong>
      <p>连接扩展后，浏览器下载和当前网页中的媒体资源可以直接发送到 RDM。</p>
    </div>
    <span class="planning">规划中</span>
  </section>

  <section class="connection-grid">
    <article>
      <div class="status-icon warning"><AppIcon name="browser" size={17} /></div>
      <div><span>浏览器扩展</span><strong>未安装</strong></div>
      <small class="warning">离线</small>
    </article>
    <article>
      <div class="status-icon"><AppIcon name="link" size={17} /></div>
      <div><span>Native Host</span><strong>等待配置</strong></div>
      <small>未连接</small>
    </article>
  </section>

  <section class="setting-group">
    <div class="group-heading">
      <div>
        <strong>下载接管</strong>
        <span>决定哪些浏览器下载交给 RDM</span>
      </div>
      <button type="button" disabled>安装扩展</button>
    </div>
    <label class="setting-row">
      <div><strong>接管常规文件下载</strong><span>压缩包、安装包、文档和媒体文件</span></div>
      <input type="checkbox" checked disabled />
    </label>
    <label class="setting-row">
      <div><strong>小文件仍由浏览器下载</strong><span>小于 1 MB 时不弹出 RDM</span></div>
      <input type="checkbox" checked disabled />
    </label>
  </section>

  <section class="setting-group">
    <div class="group-heading">
      <div>
        <strong>网页嗅探</strong>
        <span>向网页嗅探模块提供当前标签页的候选资源</span>
      </div>
    </div>
    <label class="setting-row">
      <div><strong>监听当前页面媒体</strong><span>捕获视频、音频、m3u8 和 mpd 请求</span></div>
      <input type="checkbox" checked disabled />
    </label>
    <label class="setting-row">
      <div><strong>仅连接主动选择的标签页</strong><span>减少无关请求并保持清晰的隐私边界</span></div>
      <input type="checkbox" checked disabled />
    </label>
  </section>

  <div class="boundary-note">
    <AppIcon name="shield" size={17} />
    <p><strong>职责边界：</strong>扩展负责捕获，Native Host 负责转发，下载中心统一管理最终任务。</p>
  </div>
</div>

<style>
  .browser-settings { display: flex; flex-direction: column; gap: 12px; }
  .intro {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 11px;
    padding: 13px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background:
      radial-gradient(circle at 100% 0%, var(--accent-muted), transparent 36%),
      var(--panel-deep);
  }
  .intro-icon, .status-icon {
    display: grid;
    width: 34px;
    height: 34px;
    place-items: center;
    border-radius: 9px;
    background: var(--accent-muted);
    color: var(--accent-soft);
  }
  .intro div:nth-child(2), .connection-grid article div:nth-child(2) {
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: 3px;
  }
  .intro strong { font-size: 12px; }
  .intro p { margin: 0; color: var(--muted); font-size: 10px; line-height: 1.5; }
  .planning { padding: 4px 7px; border-radius: 999px; background: var(--accent-muted); color: var(--accent-soft); font-size: 9px; }
  .connection-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .connection-grid article {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 9px;
    padding: 10px 11px;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--panel-deep);
  }
  .status-icon { width: 30px; height: 30px; }
  .status-icon.warning { background: var(--warning-muted); color: var(--warning-soft); }
  .connection-grid span { color: var(--muted); font-size: 9px; }
  .connection-grid strong { font-size: 10px; }
  .connection-grid small { padding: 3px 6px; border-radius: 999px; background: var(--panel-raised); color: var(--muted); font-size: 8px; }
  .connection-grid small.warning { background: var(--warning-muted); color: var(--warning-soft); }
  .setting-group { overflow: hidden; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-deep); }
  .group-heading, .setting-row { display: flex; align-items: center; justify-content: space-between; gap: 15px; padding: 11px 13px; }
  .group-heading { border-bottom: 1px solid var(--line); }
  .group-heading > div, .setting-row > div { display: flex; flex-direction: column; gap: 3px; }
  .group-heading strong, .setting-row strong { color: var(--text); font-size: 10px; }
  .group-heading span, .setting-row span { color: var(--muted); font-size: 9px; line-height: 1.4; }
  .group-heading button { min-height: 26px; padding: 3px 8px; font-size: 9px; }
  .setting-row { color: var(--text); }
  .setting-row + .setting-row { border-top: 1px solid var(--line); }
  .setting-row input { width: auto; accent-color: var(--accent); }
  .boundary-note { display: flex; align-items: flex-start; gap: 8px; padding: 10px 11px; border: 1px solid var(--success-muted); border-radius: 9px; background: var(--success-muted); color: var(--success-soft); }
  .boundary-note p { margin: 0; color: var(--muted); font-size: 9px; line-height: 1.55; }
  .boundary-note strong { color: var(--success); }
  @media (max-width: 640px) {
    .connection-grid { grid-template-columns: 1fr; }
  }
</style>
