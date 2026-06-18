<script>
  import { onMount, untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import AppIcon from "./AppIcon.svelte";
  import { validateSettingsForm } from "../lib/forms.js";

  let { settings, onsave, onclose } = $props();

  const ACTIVE_CHOICES = [1, 2, 3, 4, 5, 8, 10, 15, 20];
  const CONNECTION_CHOICES = [1, 2, 4, 8, 12, 16, 24, 32];
  const RETRY_CHOICES = [0, 1, 2, 3, 4, 5, 8, 10, 20];
  // Values are KB/s; 0 = unlimited.
  const SPEED_CHOICES = [0, 512, 1024, 2048, 5120, 10240, 20480, 51200, 102400];

  let downloadDir = $state(untrack(() => settings.download_dir));
  let maxActive = $state(untrack(() => settings.max_active_downloads));
  let connections = $state(untrack(() => settings.default_connections));
  let retry = $state(untrack(() => settings.retry_count));
  let speedKb = $state(untrack(() => Math.floor(settings.speed_limit_bytes / 1024)));
  let clipboard = $state(untrack(() => settings.clipboard_monitoring));
  let tray = $state(untrack(() => settings.minimize_to_tray));
  let theme = $state(untrack(() => settings.theme ?? "dark"));
  let proxyEnabled = $state(untrack(() => settings.proxy_enabled ?? false));
  let proxyUrl = $state(untrack(() => settings.proxy_url ?? ""));
  let proxyUsername = $state(untrack(() => settings.proxy_username ?? ""));
  let proxyPassword = $state(untrack(() => settings.proxy_password ?? ""));
  let error = $state("");
  let saving = $state(false);
  let section = $state("downloads");
  let dialog;

  onMount(() => dialog?.focus());

  function speedLabel(kb) {
    if (kb === 0) return "不限速";
    if (kb % 1024 === 0) return `${kb / 1024} MB/s`;
    return `${kb} KB/s`;
  }

  async function browse() {
    const picked = await open({ directory: true, defaultPath: downloadDir });
    if (picked) downloadDir = picked;
  }

  async function save(event) {
    event.preventDefault();
    error = "";
    const result = validateSettingsForm({
      downloadDir,
      maxActive,
      connections,
      retry,
      speedKb,
      clipboard,
      tray,
      theme,
      proxyEnabled,
      proxyUrl,
      proxyUsername,
      proxyPassword,
    });
    if (result.error) {
      error = result.error;
      return;
    }
    saving = true;
    try {
      await onsave(result.value);
    } catch (saveError) {
      error = String(saveError);
    } finally {
      saving = false;
    }
  }

  function handleKeydown(event) {
    if (event.key === "Escape" && !saving) onclose();
  }
</script>

<div
  class="overlay"
  onclick={(event) => event.target === event.currentTarget && !saving && onclose()}
  role="presentation"
>
  <div
    bind:this={dialog}
    class="dialog settings-dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="settings-title"
    tabindex="-1"
    onkeydown={handleKeydown}
  >
    <div class="settings-heading">
      <div>
        <h2 id="settings-title">设置</h2>
        <p class="sub">管理下载、网络和界面参数。</p>
      </div>
      <button class="close-button" type="button" aria-label="关闭设置" disabled={saving} onclick={onclose}>×</button>
    </div>

    <div class="settings-layout">
      <nav class="settings-nav" aria-label="设置分类">
        <button class:active={section === "downloads"} type="button" onclick={() => (section = "downloads")}>
          <AppIcon name="downloads" size={16} />下载设置
        </button>
        <button class:active={section === "network"} type="button" onclick={() => (section = "network")}>
          <AppIcon name="settings" size={16} />网络代理
        </button>
        <button class:active={section === "appearance"} type="button" onclick={() => (section = "appearance")}>
          <AppIcon name="settings" size={16} />外观
        </button>
      </nav>

      <div class="settings-content">
        {#if section === "downloads"}
          <form onsubmit={save}>
            <label>默认目录
              <div class="row">
                <input type="text" bind:value={downloadDir} />
                <button type="button" onclick={browse}>浏览…</button>
              </div>
            </label>
            <div class="field-grid">
              <label>同时下载
                <select bind:value={maxActive}>
                  {#each ACTIVE_CHOICES as n}
                    <option value={n}>{n} 个任务{n === 3 ? "（推荐）" : ""}</option>
                  {/each}
                </select>
              </label>
              <label>默认连接
                <select bind:value={connections}>
                  {#each CONNECTION_CHOICES as n}
                    <option value={n}>{n} 个连接{n === 8 ? "（推荐）" : ""}</option>
                  {/each}
                </select>
              </label>
              <label>失败重试
                <select bind:value={retry}>
                  {#each RETRY_CHOICES as n}
                    <option value={n}>{n === 0 ? "不重试" : `${n} 次`}{n === 4 ? "（推荐）" : ""}</option>
                  {/each}
                </select>
              </label>
              <label>全局限速
                <select bind:value={speedKb}>
                  {#each SPEED_CHOICES as kb}
                    <option value={kb}>{speedLabel(kb)}</option>
                  {/each}
                </select>
              </label>
            </div>
            <div class="behavior-group">
              <label class="check">
                <input type="checkbox" bind:checked={clipboard} />
                <span><strong>剪贴板监控</strong><small>自动识别复制的 HTTP/HTTPS 地址</small></span>
              </label>
              <label class="check">
                <input type="checkbox" bind:checked={tray} />
                <span><strong>最小化到托盘</strong><small>关闭主窗口后继续运行下载任务</small></span>
              </label>
            </div>
            {#if error}<p class="error">{error}</p>{/if}
            <div class="actions">
              <button type="button" class="ghost" disabled={saving} onclick={onclose}>取消</button>
              <button type="submit" class="primary" disabled={saving}>
                {saving ? "正在保存…" : "保存设置"}
              </button>
            </div>
          </form>
        {:else if section === "network"}
          <form onsubmit={save}>
            <div class="section-title">
              <strong>代理服务器</strong>
              <span>通过代理访问需要 VPN 的链接，支持 http://、https:// 与 socks5://。仅对保存后新建的下载与解析生效。</span>
            </div>
            <label class="check proxy-toggle">
              <input type="checkbox" bind:checked={proxyEnabled} />
              <span><strong>启用代理</strong><small>开启后所有请求经由代理地址转发</small></span>
            </label>
            <div class="proxy-fields" class:disabled={!proxyEnabled}>
              <label>代理地址
                <input
                  type="text"
                  bind:value={proxyUrl}
                  placeholder="http://127.0.0.1:7890"
                  disabled={!proxyEnabled}
                  autocomplete="off"
                  spellcheck="false"
                />
              </label>
              <label>用户名
                <input
                  type="text"
                  bind:value={proxyUsername}
                  placeholder="可选"
                  disabled={!proxyEnabled}
                  autocomplete="off"
                />
              </label>
              <label>密码
                <input
                  type="password"
                  bind:value={proxyPassword}
                  placeholder="可选"
                  disabled={!proxyEnabled}
                  autocomplete="new-password"
                />
              </label>
            </div>
            {#if error}<p class="error">{error}</p>{/if}
            <div class="actions">
              <button type="button" class="ghost" disabled={saving} onclick={onclose}>取消</button>
              <button type="submit" class="primary" disabled={saving}>
                {saving ? "正在保存…" : "保存设置"}
              </button>
            </div>
          </form>
        {:else if section === "appearance"}
          <form onsubmit={save}>
            <div class="section-title">
              <strong>主题</strong>
              <span>选择应用界面的颜色方案。</span>
            </div>
            <div class="theme-grid">
              <label class:active={theme === "light"} class="theme-option">
                <input type="radio" bind:group={theme} value="light" />
                <span class="theme-preview light-preview">
                  <i></i><b></b><em></em>
                </span>
                <span><strong>亮色</strong><small>明亮背景，适合白天使用</small></span>
              </label>
              <label class:active={theme === "dark"} class="theme-option">
                <input type="radio" bind:group={theme} value="dark" />
                <span class="theme-preview dark-preview">
                  <i></i><b></b><em></em>
                </span>
                <span><strong>暗色</strong><small>低亮度背景，适合夜间使用</small></span>
              </label>
            </div>
            {#if error}<p class="error">{error}</p>{/if}
            <div class="actions appearance-actions">
              <button type="button" class="ghost" disabled={saving} onclick={onclose}>取消</button>
              <button type="submit" class="primary" disabled={saving}>
                {saving ? "正在保存…" : "保存设置"}
              </button>
            </div>
          </form>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .settings-heading { display: flex; align-items: flex-start; justify-content: space-between; }
  .settings-dialog { width: 700px; }
  .settings-heading .sub { margin-bottom: 14px; }
  .close-button { min-width: 28px; min-height: 28px; padding: 0; border-color: transparent; background: transparent; color: var(--muted); font-size: 20px; }
  .settings-layout { display: grid; grid-template-columns: 150px minmax(0, 1fr); min-height: 440px; overflow: hidden; border: 1px solid var(--line); border-radius: 11px; }
  .settings-nav { display: flex; flex-direction: column; gap: 4px; padding: 10px; border-right: 1px solid var(--line); background: var(--panel-deep); }
  .settings-nav button { display: grid; grid-template-columns: 20px 1fr auto; align-items: center; gap: 7px; width: 100%; border-color: transparent; background: transparent; color: var(--muted); text-align: left; }
  .settings-nav button:hover { border-color: transparent; }
  .settings-nav button.active { background: var(--accent-muted); color: var(--text); }
  .settings-content { min-width: 0; max-height: min(560px, calc(100vh - 130px)); overflow: auto; padding: 17px; }
  .field-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .behavior-group { overflow: hidden; border: 1px solid var(--line); border-radius: 9px; background: var(--panel-deep); }
  .behavior-group .check { padding: 10px 11px; }
  .behavior-group .check + .check { border-top: 1px solid var(--line); }
  .behavior-group .check span { display: flex; flex-direction: column; gap: 3px; }
  .behavior-group .check strong { color: var(--text); font-size: 10px; font-weight: 600; }
  .behavior-group .check small { color: var(--muted); font-size: 9px; }
  .section-title { display: flex; flex-direction: column; gap: 4px; margin-bottom: 14px; }
  .section-title strong { color: var(--text); font-size: 13px; }
  .section-title span { color: var(--muted); font-size: 10px; }
  .proxy-toggle { display: flex; align-items: center; gap: 9px; padding: 10px 11px; margin-bottom: 12px; border: 1px solid var(--line); border-radius: 9px; background: var(--panel-deep); }
  .proxy-toggle span { display: flex; flex-direction: column; gap: 3px; }
  .proxy-toggle strong { color: var(--text); font-size: 10px; font-weight: 600; }
  .proxy-toggle small { color: var(--muted); font-size: 9px; }
  .proxy-fields { display: grid; grid-template-columns: 1fr; gap: 12px; transition: opacity 0.15s ease; }
  .proxy-fields.disabled { opacity: 0.5; }
  .proxy-fields label { display: flex; flex-direction: column; gap: 4px; }
  .theme-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .theme-option { position: relative; padding: 10px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-deep); cursor: pointer; }
  .theme-option.active { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-muted); }
  .theme-option > input { position: absolute; opacity: 0; pointer-events: none; }
  .theme-option > span:last-child { display: flex; flex-direction: column; gap: 3px; padding: 9px 2px 2px; }
  .theme-option strong { color: var(--text); font-size: 11px; }
  .theme-option small { color: var(--muted); font-size: 9px; }
  .theme-preview { position: relative; display: block; height: 92px; overflow: hidden; border: 1px solid var(--line); border-radius: 7px; }
  .theme-preview i { position: absolute; width: 25%; inset: 0 auto 0 0; }
  .theme-preview b, .theme-preview em { position: absolute; left: 32%; right: 8%; border-radius: 4px; }
  .theme-preview b { height: 18px; top: 14px; }
  .theme-preview em { height: 38px; top: 40px; }
  .light-preview { background: var(--bg); }
  .light-preview i { background: var(--sidebar); border-right: 1px solid var(--sidebar-line); }
  .light-preview b, .light-preview em { background: var(--panel); border: 1px solid var(--line); }
  .dark-preview { background: var(--bg); }
  .dark-preview i { background: var(--sidebar); border-right: 1px solid var(--sidebar-line); }
  .dark-preview b, .dark-preview em { background: var(--panel); border: 1px solid var(--line); }
  .appearance-actions { margin-top: 18px !important; }
  @media (max-width: 720px) {
    .settings-layout { grid-template-columns: 1fr; }
    .settings-nav { flex-direction: row; border-right: 0; border-bottom: 1px solid var(--line); }
    .field-grid { grid-template-columns: 1fr; }
  }
</style>
