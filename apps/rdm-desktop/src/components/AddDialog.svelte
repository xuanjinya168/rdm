<script>
  import { onMount, untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { validateDownloadForm } from "../lib/forms.js";

  let { settings, initialUrl = "", onsubmit, onclose } = $props();

  const CONNECTION_CHOICES = [1, 2, 4, 8, 12, 16, 24, 32];

  let url = $state(untrack(() => initialUrl));
  let destination = $state(untrack(() => settings.download_dir));
  let filename = $state("");
  let connections = $state(untrack(() => settings.default_connections));
  let sha256 = $state("");
  let error = $state("");
  let submitting = $state(false);
  let dialog;

  onMount(() => dialog?.focus());

  async function browse() {
    const picked = await open({ directory: true, defaultPath: destination });
    if (picked) destination = picked;
  }

  async function submit(event) {
    event.preventDefault();
    error = "";
    const result = validateDownloadForm({ url, destination, filename, connections, sha256 });
    if (result.error) {
      error = result.error;
      return;
    }
    submitting = true;
    try {
      await onsubmit(result.value);
    } catch (submitError) {
      error = String(submitError);
    } finally {
      submitting = false;
    }
  }

  function handleKeydown(event) {
    if (event.key === "Escape" && !submitting) onclose();
  }
</script>

<div
  class="overlay"
  onclick={(event) => event.target === event.currentTarget && !submitting && onclose()}
  role="presentation"
>
  <div
    bind:this={dialog}
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="add-title"
    tabindex="-1"
    onkeydown={handleKeydown}
  >
    <h2 id="add-title">新建下载</h2>
    <p class="sub">添加下载地址并选择文件保存位置。</p>
    <form onsubmit={submit}>
      <label>下载地址
        <input type="url" bind:value={url} placeholder="https://example.com/file.zip" />
      </label>
      <label>保存位置
        <div class="row">
          <input type="text" bind:value={destination} />
          <button type="button" onclick={browse}>浏览…</button>
        </div>
      </label>
      <label>文件名称
        <input type="text" bind:value={filename} placeholder="留空则从服务器自动识别" />
      </label>
      <label>并发连接
        <select bind:value={connections}>
          {#each CONNECTION_CHOICES as n}
            <option value={n}>{n} 个连接{n === 8 ? "（推荐）" : ""}</option>
          {/each}
        </select>
      </label>
      <label>SHA-256
        <input type="text" maxlength="64" bind:value={sha256} placeholder="可选，64 位十六进制" />
      </label>
      <p class="hint">服务器不支持分段时会自动降级为单连接；填写 SHA-256 后将在完成前校验文件。</p>
      {#if error}<p class="error">{error}</p>{/if}
      <div class="actions">
        <button type="button" class="ghost" disabled={submitting} onclick={onclose}>取消</button>
        <button type="submit" class="primary" disabled={submitting}>
          {submitting ? "正在添加…" : "开始下载"}
        </button>
      </div>
    </form>
  </div>
</div>
