<script>
  import AppIcon from "./AppIcon.svelte";

  let activeKind = $state("all");
  const filters = [
    ["all", "全部"],
    ["video", "视频"],
    ["audio", "音频"],
    ["manifest", "流媒体"],
  ];
</script>

<div class="sniffer-page">
  <section class="connection-card">
    <div class="connection-copy">
      <span class="status-light"></span>
      <div>
        <strong>浏览器嗅探器未连接</strong>
        <p>安装扩展并连接 Native Host 后，当前标签页的媒体请求会显示在这里。</p>
      </div>
    </div>
    <button disabled title="浏览器扩展尚未接入">等待扩展连接</button>
  </section>

  <section class="sniffer-workspace">
    <aside class="session-panel">
      <div class="panel-title">
        <div>
          <span>浏览器会话</span>
          <strong>当前标签页</strong>
        </div>
        <span class="counter">0</span>
      </div>
      <div class="page-placeholder">
        <div class="browser-symbol"><AppIcon name="browser" size={22} /></div>
        <strong>尚无活动页面</strong>
        <span>连接浏览器后自动同步页面标题和地址</span>
      </div>
      <div class="session-options">
        <label><input type="checkbox" checked disabled /> 捕获媒体请求</label>
        <label><input type="checkbox" checked disabled /> 合并重复地址</label>
        <label><input type="checkbox" disabled /> 显示图片资源</label>
      </div>
    </aside>

    <div class="resource-panel">
      <div class="resource-toolbar">
        <div class="filter-list">
          {#each filters as [key, label]}
            <button class:active={activeKind === key} onclick={() => (activeKind = key)}>{label}</button>
          {/each}
        </div>
        <div class="toolbar-actions">
          <button disabled><AppIcon name="filter" size={14} /> 筛选</button>
          <button disabled>清空</button>
        </div>
      </div>

      <div class="resource-head">
        <span>资源</span><span>类型</span><span>大小</span><span>来源</span><span></span>
      </div>
      <div class="resource-empty">
        <div class="radar">
          <span class="radar-ring one"></span>
          <span class="radar-ring two"></span>
          <span class="radar-core"><AppIcon name="sniff" size={20} /></span>
        </div>
        <strong>等待发现媒体资源</strong>
        <p>播放网页中的视频或音频，捕获到的直链、m3u8 和 mpd 会实时出现。</p>
      </div>
    </div>
  </section>

  <section class="rule-strip">
    <div><AppIcon name="shield" size={18} /><span><strong>隐私边界</strong>仅处理你主动连接的标签页</span></div>
    <div><AppIcon name="activity" size={18} /><span><strong>实时归类</strong>按 MIME、扩展名和请求特征识别</span></div>
    <div><AppIcon name="copy" size={18} /><span><strong>一键建任务</strong>保留必要请求头并发送给下载引擎</span></div>
  </section>
</div>

<style>
  .sniffer-page { display: flex; flex-direction: column; gap: 16px; }
  .connection-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 17px 19px;
    border: 1px solid var(--warning-muted);
    border-radius: var(--radius-lg);
    background: linear-gradient(90deg, var(--warning-muted), var(--panel));
  }
  .connection-copy { display: flex; align-items: flex-start; gap: 12px; }
  .status-light { width: 8px; height: 8px; margin-top: 5px; border-radius: 50%; background: var(--warning); box-shadow: 0 0 0 5px var(--warning-muted); }
  .connection-copy strong { display: block; font-size: 13px; }
  .connection-copy p { margin: 5px 0 0; color: var(--muted); font-size: 12px; }
  .sniffer-workspace {
    display: grid;
    grid-template-columns: 240px 1fr;
    min-height: 455px;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel);
  }
  .session-panel { display: flex; flex-direction: column; border-right: 1px solid var(--line); background: var(--panel-deep); }
  .panel-title { display: flex; align-items: center; justify-content: space-between; padding: 17px; border-bottom: 1px solid var(--line); }
  .panel-title div { display: flex; flex-direction: column; gap: 3px; }
  .panel-title span { color: var(--muted); font-size: 10px; text-transform: uppercase; letter-spacing: 0.08em; }
  .panel-title strong { font-size: 13px; }
  .counter { display: grid; width: 23px; height: 23px; place-items: center; border-radius: 7px; background: var(--panel-raised); color: var(--muted); }
  .page-placeholder { display: flex; flex: 1; flex-direction: column; align-items: center; justify-content: center; padding: 25px; text-align: center; }
  .browser-symbol { display: grid; width: 46px; height: 46px; place-items: center; border: 1px solid var(--line); border-radius: 13px; background: var(--panel-raised); color: var(--muted); }
  .page-placeholder strong { margin-top: 13px; font-size: 12px; }
  .page-placeholder span { max-width: 170px; margin-top: 5px; color: var(--muted); font-size: 10px; line-height: 1.6; }
  .session-options { display: grid; gap: 11px; padding: 15px 17px; border-top: 1px solid var(--line); }
  .session-options label { display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 11px; }
  .session-options input { accent-color: var(--accent); }
  .resource-panel { display: flex; min-width: 0; flex-direction: column; }
  .resource-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 12px 14px; border-bottom: 1px solid var(--line); }
  .filter-list { display: flex; gap: 5px; }
  .filter-list button { padding: 5px 10px; border-color: transparent; background: transparent; color: var(--muted); }
  .filter-list button.active { border-color: var(--line); background: var(--panel-raised); color: var(--text); }
  .toolbar-actions { display: flex; gap: 6px; }
  .toolbar-actions button { display: flex; align-items: center; gap: 5px; padding: 5px 9px; font-size: 11px; }
  .resource-head { display: grid; grid-template-columns: minmax(220px, 1fr) 90px 80px 110px 30px; gap: 10px; padding: 10px 15px; border-bottom: 1px solid var(--line); color: var(--muted); font-size: 10px; }
  .resource-empty { display: flex; flex: 1; flex-direction: column; align-items: center; justify-content: center; padding: 30px; text-align: center; }
  .resource-empty strong { margin-top: 15px; font-size: 13px; }
  .resource-empty p { max-width: 360px; margin: 7px 0 0; color: var(--muted); font-size: 11px; line-height: 1.6; }
  .radar { position: relative; display: grid; width: 74px; height: 74px; place-items: center; }
  .radar-ring { position: absolute; border: 1px solid var(--accent-muted); border-radius: 50%; }
  .radar-ring.one { inset: 10px; }
  .radar-ring.two { inset: 0; }
  .radar-core { display: grid; width: 37px; height: 37px; place-items: center; border-radius: 50%; background: var(--accent-muted); color: var(--accent-soft); }
  .rule-strip { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
  .rule-strip > div { display: flex; align-items: center; gap: 11px; min-width: 0; padding: 13px 14px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel); color: var(--accent-soft); }
  .rule-strip span { min-width: 0; color: var(--muted); font-size: 10px; line-height: 1.5; }
  .rule-strip strong { display: block; color: var(--text); font-size: 11px; }
  @media (max-width: 900px) {
    .sniffer-workspace { grid-template-columns: 1fr; }
    .session-panel {
      display: grid;
      grid-template-columns: 140px 1fr 160px;
      min-height: 140px;
      border-right: 0;
      border-bottom: 1px solid var(--line);
    }
    .panel-title { border-right: 1px solid var(--line); border-bottom: 0; }
    .page-placeholder { padding: 14px; }
    .browser-symbol { width: 38px; height: 38px; }
    .page-placeholder strong { margin-top: 8px; }
    .page-placeholder span { display: none; }
    .session-options { align-content: center; padding: 12px; border-top: 0; border-left: 1px solid var(--line); }
    .resource-head { grid-template-columns: minmax(160px, 1fr) 70px 60px 90px 24px; }
    .rule-strip { grid-template-columns: 1fr; }
  }
</style>
