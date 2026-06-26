<script>
  import { onMount, untrack } from "svelte";
  import AppIcon from "./AppIcon.svelte";
  import { formatBytes } from "../lib/format.js";

  // media: { candidates: [{ url, filename?, kind?, ext?, width?, height?, duration?, bytes? }], pageTitle? }
  // onDownload(values) 把一个媒体项加入下载队列；onclose() 关闭对话框。
  let { media, downloadDir = "", onDownload, onclose } = $props();

  const KIND_LABEL = { image: "图片", video: "视频", audio: "音频", manifest: "流媒体" };

  const items = untrack(() => media?.candidates ?? []);
  // manifest（m3u8/mpd）暂仅识别展示，不可下载。
  const downloadableIndexes = items
    .map((c, i) => (c.kind === "manifest" ? -1 : i))
    .filter((i) => i >= 0);

  let selected = $state(untrack(() => new Set(downloadableIndexes)));
  let queued = $state(new Set());
  let busy = $state(new Set());
  let message = $state("");
  let dialog;

  onMount(() => dialog?.focus());

  const selectableCount = downloadableIndexes.length;
  const selectedCount = $derived([...selected].filter((i) => !queued.has(i)).length);
  const allSelected = $derived(
    selectableCount > 0 && downloadableIndexes.every((i) => selected.has(i) || queued.has(i)),
  );
  const working = $derived(busy.size > 0);

  function toggle(index, on) {
    const next = new Set(selected);
    if (on) next.add(index);
    else next.delete(index);
    selected = next;
  }

  function toggleAll(on) {
    const next = new Set();
    if (on) for (const i of downloadableIndexes) if (!queued.has(i)) next.add(i);
    selected = next;
  }

  function metaText(c) {
    const parts = [];
    if (c.ext) parts.push(c.ext.toUpperCase());
    if (c.width && c.height) parts.push(`${c.width}×${c.height}`);
    if (c.duration) parts.push(formatDuration(c.duration));
    if (c.bytes) parts.push(formatBytes(c.bytes));
    return parts.join(" · ");
  }

  function formatDuration(sec) {
    const s = Math.round(sec);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }

  // 返回 true/false 表示该项是否成功加入下载，供批量统计使用。
  async function downloadOne(index) {
    const c = items[index];
    if (!c || queued.has(index) || busy.has(index)) return false;
    busy = new Set(busy).add(index);
    try {
      await onDownload({ url: c.url, filename: c.filename });
      queued = new Set(queued).add(index);
      const next = new Set(selected);
      next.delete(index);
      selected = next;
      return true;
    } catch (error) {
      message = `「${c.filename || c.url}」加入下载失败：${error}`;
      return false;
    } finally {
      const next = new Set(busy);
      next.delete(index);
      busy = next;
    }
  }

  async function downloadSelected() {
    const targets = [...selected].filter((i) => !queued.has(i)).sort((a, b) => a - b);
    if (targets.length === 0) return;
    message = "";
    let ok = 0;
    let fail = 0;
    for (const i of targets) {
      if (await downloadOne(i)) ok += 1;
      else fail += 1;
    }
    // 有失败就如实汇报，不用「全部成功」覆盖单项失败。
    message = fail === 0 ? `已将 ${ok} 项加入下载中心。` : `已加入 ${ok} 项，失败 ${fail} 项。`;
  }

  function handleKeydown(event) {
    if (event.key === "Escape" && !working) onclose();
  }
</script>

<div
  class="overlay"
  role="presentation"
  onclick={(e) => e.target === e.currentTarget && !working && onclose()}
>
  <div
    bind:this={dialog}
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="sniff-title"
    tabindex="-1"
    onkeydown={handleKeydown}
  >
    <h2 id="sniff-title">嗅探到的媒体（{items.length}）</h2>
    <p class="sub">
      {media?.pageTitle ? media.pageTitle : "勾选要下载的资源，点「下载所选」加入下载中心。"}
    </p>

    <div class="sniff-toolbar">
      <label class="check">
        <input
          type="checkbox"
          checked={allSelected}
          disabled={selectableCount === 0}
          onchange={(e) => toggleAll(e.currentTarget.checked)}
        />
        全选可下载
      </label>
      <span class="count">已选 {selectedCount} / {selectableCount}</span>
    </div>

    <div class="sniff-list">
      {#each items as c, index}
        <article class="sniff-item" class:done={queued.has(index)}>
          <input
            type="checkbox"
            aria-label={`选择 ${c.filename || c.url}`}
            checked={selected.has(index)}
            disabled={c.kind === "manifest" || queued.has(index) || busy.has(index)}
            onchange={(e) => toggle(index, e.currentTarget.checked)}
          />
          <div class="item-body">
            <div class="item-top">
              <span class="kind kind-{c.kind || 'other'}">{KIND_LABEL[c.kind] || c.kind || "资源"}</span>
              <strong title={c.url}>{c.filename || c.url}</strong>
            </div>
            <small>{metaText(c)}{#if c.kind === "manifest"} · 仅识别，暂不支持下载{/if}</small>
            <small class="item-url" title={c.url}>{c.url}</small>
          </div>
          {#if queued.has(index)}
            <span class="item-state done"><AppIcon name="check" size={14} /> 已加入</span>
          {:else if busy.has(index)}
            <span class="item-state">添加中…</span>
          {:else if c.kind === "manifest"}
            <span class="item-state muted">仅识别</span>
          {:else}
            <button class="row-dl" onclick={() => downloadOne(index)}>下载</button>
          {/if}
        </article>
      {/each}
    </div>

    {#if downloadDir}
      <p class="hint">下载将保存到默认目录：{downloadDir}</p>
    {/if}
    {#if message}<p class="msg">{message}</p>{/if}

    <div class="actions">
      <button type="button" class="ghost" onclick={onclose} disabled={working}>关闭</button>
      <button
        type="button"
        class="primary"
        onclick={downloadSelected}
        disabled={working || selectedCount === 0}
      >
        下载所选（{selectedCount}）
      </button>
    </div>
  </div>
</div>

<style>
  .sniff-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .sniff-toolbar .count {
    color: var(--muted);
    font-size: 11px;
  }
  .sniff-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 50vh;
    overflow-y: auto;
  }
  .sniff-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--panel-deep);
  }
  .sniff-item.done {
    border-color: var(--success-muted);
  }
  .item-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .item-top {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
  }
  .item-top strong {
    overflow: hidden;
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item-body small {
    color: var(--muted);
    font-size: 11px;
  }
  .item-url {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    opacity: 0.7;
    font-size: 10px;
  }
  .kind {
    flex: none;
    padding: 1px 7px;
    border-radius: 5px;
    font-size: 10px;
    font-weight: 600;
    background: var(--accent-muted);
    color: var(--accent-soft);
  }
  .kind-image {
    background: var(--success-muted);
    color: var(--success-soft);
  }
  .kind-audio {
    background: var(--info-muted);
    color: var(--info-soft);
  }
  .kind-manifest {
    background: var(--danger-muted);
    color: var(--danger-soft);
  }
  .item-state {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 11px;
    color: var(--muted);
  }
  .item-state.done {
    color: var(--success-soft);
  }
  .item-state.muted {
    opacity: 0.7;
  }
  .row-dl {
    flex: none;
    padding: 6px 12px;
    border: 1px solid var(--accent);
    border-radius: 7px;
    background: var(--accent-muted);
    color: var(--accent-soft);
    font-size: 12px;
    font-weight: 600;
  }
  .msg {
    margin: 8px 0 0;
    color: var(--muted);
    font-size: 11px;
  }
</style>
