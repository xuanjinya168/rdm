<script>
  import AppIcon from "./AppIcon.svelte";
  import { isHttpUrl } from "../lib/validate.js";
  import { resolveMedia } from "../lib/api.js";

  // onDownload(values) -> queues one media item and returns the created task.
  let { onDownload, downloadDir = "" } = $props();

  let url = $state("");
  let loading = $state(false);
  let message = $state("");
  let messageType = $state("info");
  let post = $state(null);
  let queued = $state(new Set()); // indices already sent to the download engine
  let busy = $state(new Set()); // indices currently being queued

  function setMessage(text, type = "info") {
    message = text;
    messageType = type;
  }

  async function resolve(event) {
    event.preventDefault();
    const value = url.trim();
    if (!isHttpUrl(value)) {
      setMessage("请输入有效的网页或媒体地址。", "error");
      return;
    }
    loading = true;
    post = null;
    queued = new Set();
    setMessage("正在解析链接…", "info");
    try {
      const result = await resolveMedia(value);
      post = result;
      setMessage(`解析成功，找到 ${result.media.length} 个媒体文件。`, "success");
    } catch (error) {
      setMessage(String(error), "error");
    } finally {
      loading = false;
    }
  }

  async function download(item, index) {
    if (queued.has(index) || busy.has(index)) return;
    busy = new Set(busy).add(index);
    try {
      await onDownload?.({ url: item.url, filename: item.filename });
      queued = new Set(queued).add(index);
    } catch (error) {
      setMessage(`「${item.filename}」加入下载失败：${error}`, "error");
    } finally {
      const next = new Set(busy);
      next.delete(index);
      busy = next;
    }
  }

  async function downloadAll() {
    if (!post) return;
    for (let i = 0; i < post.media.length; i += 1) {
      await download(post.media[i], i);
    }
    setMessage("已将全部媒体加入下载中心。", "success");
  }

</script>

<div class="media-page">
  <section class="resolver-hero">
    <form class="resolver-box" onsubmit={resolve}>
      <div class="input-wrap">
        <AppIcon name="link" size={18} />
        <input bind:value={url} aria-label="待解析地址" placeholder="粘贴 Twitter / X、Instagram 或 Threads 帖子地址" />
        {#if url}
          <button type="button" class="clear" aria-label="清空地址" onclick={() => (url = "")}>×</button>
        {/if}
      </div>
      <button class="primary resolve-button" type="submit" disabled={loading}>
        {loading ? "解析中…" : "解析"}
      </button>
    </form>

    {#if message}
      <div
        class="prototype-note"
        class:message-error={messageType === "error"}
        class:message-success={messageType === "success"}
      >
        <span class="note-dot"></span>{message}
      </div>
    {/if}
  </section>

  <section class="section-block">
    <div class="section-heading">
      <div>
        <span class="section-kicker">解析结果</span>
        <h3>媒体选择器</h3>
      </div>
      {#if post && post.media.length}
        <button class="primary small" onclick={downloadAll}>
          <AppIcon name="downloads" size={14} /> 全部下载
        </button>
      {/if}
    </div>

    {#if post}
      {#if post.title}
        <h4 class="post-title">{post.title}</h4>
      {/if}
      {#if post.text}
        <p class="post-text">{post.text}</p>
      {/if}
      <div class="media-grid">
        {#each post.media as item, index}
          <article class="media-card">
            <div class="media-meta">
              <strong title={item.filename}>{item.filename}</strong>
              <small>
                {item.ext.toUpperCase()}{#if item.width && item.height} · {item.width}×{item.height}{/if}
              </small>
            </div>
            <button
              class="download-btn"
              class:done={queued.has(index)}
              disabled={queued.has(index) || busy.has(index)}
              onclick={() => download(item, index)}
            >
              {#if queued.has(index)}
                <AppIcon name="check" size={14} /> 已加入
              {:else if busy.has(index)}
                添加中…
              {:else}
                <AppIcon name="downloads" size={14} /> 下载
              {/if}
            </button>
          </article>
        {/each}
      </div>
      {#if downloadDir}
        <p class="save-hint">下载将保存到默认目录：{downloadDir}</p>
      {/if}
    {:else}
      <div class="result-placeholder">
        <div class="empty-overlay">
          <AppIcon name="media" size={25} />
          <strong>解析结果会显示在这里</strong>
          <span>粘贴帖子地址并点击「解析」</span>
        </div>
      </div>
    {/if}
  </section>

</div>

<style>
  .media-page { display: flex; flex-direction: column; gap: 18px; }
  .resolver-hero {
    position: relative;
    overflow: hidden;
    padding: 13px 15px;
    border: 1px solid var(--line);
    border-radius: 11px;
    background:
      radial-gradient(circle at 90% 0%, var(--accent-muted), transparent 34%),
      linear-gradient(135deg, var(--panel), var(--panel-deep));
  }
  .resolver-hero::after {
    position: absolute;
    width: 230px;
    height: 230px;
    right: -90px;
    bottom: -150px;
    border: 1px solid var(--accent-muted);
    border-radius: 50%;
    content: "";
  }
  .resolver-box {
    position: relative;
    z-index: 1;
    display: flex;
    max-width: 760px;
    gap: 10px;
  }
  .input-wrap {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 0 13px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: color-mix(in srgb, var(--panel-deep) 88%, transparent);
    color: var(--muted);
  }
  .input-wrap:focus-within { border-color: var(--accent); box-shadow: 0 0 0 3px var(--focus-shadow); }
  .input-wrap input {
    flex: 1;
    min-width: 0;
    padding: 12px 0;
    border: 0;
    background: transparent;
    box-shadow: none;
  }
  .clear { padding: 2px 5px; border: 0; background: transparent; color: var(--muted); font-size: 18px; }
  .resolve-button { min-width: 105px; }
  .prototype-note {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 760px;
    margin-top: 14px;
    padding: 9px 11px;
    border: 1px solid var(--info-muted);
    border-radius: 8px;
    background: var(--info-muted);
    color: var(--info-soft);
    font-size: 12px;
  }
  .prototype-note.message-error { border-color: var(--danger-muted); background: var(--danger-muted); color: var(--danger-soft); }
  .prototype-note.message-success { border-color: var(--success-muted); background: var(--success-muted); color: var(--success-soft); }
  .note-dot { width: 6px; height: 6px; border-radius: 50%; background: currentColor; }
  .section-block {
    padding: 20px;
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel);
  }
  .section-heading { display: flex; align-items: center; justify-content: space-between; margin-bottom: 16px; }
  .section-kicker { color: var(--muted); font-size: 11px; font-weight: 650; letter-spacing: 0.08em; text-transform: uppercase; }
  h3 { margin: 3px 0 0; font-size: 16px; }
  .primary.small { display: inline-flex; align-items: center; gap: 6px; min-width: 0; padding: 7px 13px; font-size: 12px; }
  .post-title { margin: 0 0 8px; font-size: 15px; }
  .post-text { margin: 0 0 16px; color: var(--muted); font-size: 13px; line-height: 1.7; white-space: pre-wrap; word-break: break-word; }
  .media-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(190px, 1fr)); gap: 12px; }
  .media-card {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--panel-deep);
  }
  .media-meta { display: flex; flex-direction: column; gap: 3px; padding: 10px 12px 4px; min-width: 0; }
  .media-meta strong { overflow: hidden; font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
  .media-meta small { color: var(--muted); font-size: 11px; }
  .download-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin: 8px 12px 12px;
    padding: 8px 0;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--accent-muted);
    color: var(--accent-soft);
    font-size: 12px;
    font-weight: 600;
  }
  .download-btn:disabled { opacity: 0.75; }
  .download-btn.done { border-color: var(--success-muted); background: var(--success-muted); color: var(--success-soft); }
  .save-hint { margin: 14px 0 0; color: var(--muted); font-size: 11px; }
  .result-placeholder {
    position: relative;
    display: grid;
    min-height: 200px;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--panel-deep);
  }
  .empty-overlay {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }
  .empty-overlay strong { margin: 10px 0 4px; color: var(--text); font-size: 13px; }
  .empty-overlay span { font-size: 11px; }
</style>
