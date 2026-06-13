<script>
  import { open } from "@tauri-apps/plugin-dialog";
  import { isHttpUrl, isValidWindowsFilename, normalizeSha256 } from "../lib/validate.js";

  let { settings, initialUrl = "", onsubmit, onclose } = $props();

  const CONNECTION_CHOICES = [1, 2, 4, 8, 12, 16, 24, 32];

  let url = $state(initialUrl);
  let destination = $state(settings.download_dir);
  let filename = $state("");
  let connections = $state(settings.default_connections);
  let sha256 = $state("");
  let error = $state("");

  async function browse() {
    const picked = await open({ directory: true, defaultPath: destination });
    if (picked) destination = picked;
  }

  function submit(event) {
    event.preventDefault();
    error = "";
    const trimmedUrl = url.trim();
    if (!isHttpUrl(trimmedUrl)) {
      error = "请输入有效的 HTTP 或 HTTPS 地址。";
      return;
    }
    if (!destination.trim()) {
      error = "请选择保存目录。";
      return;
    }
    if (filename.trim() && !isValidWindowsFilename(filename.trim())) {
      error = "文件名不符合 Windows 命名规则。";
      return;
    }
    const checksum = normalizeSha256(sha256);
    if (checksum.error) {
      error = checksum.error;
      return;
    }
    onsubmit({
      url: trimmedUrl,
      destination: destination.trim(),
      connections: Number(connections),
      filename: filename.trim(),
      sha256: checksum.value ?? "",
    });
  }
</script>

<div class="overlay" onclick={onclose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
    <h2>新建下载</h2>
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
        <button type="button" class="ghost" onclick={onclose}>取消</button>
        <button type="submit" class="primary">开始下载</button>
      </div>
    </form>
  </div>
</div>
