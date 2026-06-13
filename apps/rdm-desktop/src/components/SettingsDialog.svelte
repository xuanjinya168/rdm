<script>
  import { onMount, untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";

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
  let error = $state("");
  let saving = $state(false);
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
    if (!downloadDir.trim()) {
      error = "请选择默认下载目录。";
      return;
    }
    saving = true;
    try {
      await onsave({
        download_dir: downloadDir.trim(),
        max_active_downloads: Number(maxActive),
        default_connections: Number(connections),
        retry_count: Number(retry),
        speed_limit_bytes: Number(speedKb) * 1024,
        clipboard_monitoring: clipboard,
        minimize_to_tray: tray,
      });
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
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="settings-title"
    tabindex="-1"
    onkeydown={handleKeydown}
  >
    <h2 id="settings-title">下载设置</h2>
    <p class="sub">调整并发数量、速度限制和桌面行为。</p>
    <form onsubmit={save}>
      <label>默认目录
        <div class="row">
          <input type="text" bind:value={downloadDir} />
          <button type="button" onclick={browse}>浏览…</button>
        </div>
      </label>
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
      <label class="check">
        <input type="checkbox" bind:checked={clipboard} />
        自动识别剪贴板中的 HTTP/HTTPS 地址
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={tray} />
        关闭主窗口时继续在系统托盘运行
      </label>
      {#if error}<p class="error">{error}</p>{/if}
      <div class="actions">
        <button type="button" class="ghost" disabled={saving} onclick={onclose}>取消</button>
        <button type="submit" class="primary" disabled={saving}>
          {saving ? "正在保存…" : "保存设置"}
        </button>
      </div>
    </form>
  </div>
</div>
