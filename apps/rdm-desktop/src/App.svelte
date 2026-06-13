<script>
  import { onMount } from "svelte";
  import { readText } from "@tauri-apps/plugin-clipboard-manager";
  import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
  } from "@tauri-apps/plugin-notification";
  import AddDialog from "./components/AddDialog.svelte";
  import SettingsDialog from "./components/SettingsDialog.svelte";
  import {
    listTasks,
    getSettings,
    saveSettings,
    addDownload,
    startTask,
    pauseTask,
    cancelTask,
    deleteTask,
    openFolder,
    onTaskUpdate,
    onOpenUrl,
    onNewDownload,
  } from "./lib/api.js";
  import {
    formatBytes,
    formatSpeed,
    formatEta,
    percent,
    statusLabel,
    ACTIVE_STATUSES,
    ACTIVE_FILTER_STATUSES,
  } from "./lib/format.js";
  import { mergeTaskSnapshots } from "./lib/tasks.js";
  import { isHttpUrl } from "./lib/validate.js";

  let tasks = $state([]);
  let speeds = $state({});
  let settings = $state(null);
  let filter = $state("all");
  let selectedId = $state(null);
  let quickUrl = $state("");
  let quickError = $state("");

  let addOpen = $state(false);
  let addUrl = $state("");
  let settingsOpen = $state(false);
  let deleteTarget = $state(null);
  let deleteCancel = $state();
  let menu = $state(null); // { task, x, y }

  let notifyOk = false;
  let lastClipboard = "";

  const sorted = $derived([...tasks].sort((a, b) => b.created_at - a.created_at));
  const visible = $derived(sorted.filter(matchesFilter));
  const stats = $derived({
    total: tasks.length,
    active: tasks.filter((t) => ACTIVE_STATUSES.has(t.status)).length,
    completed: tasks.filter((t) => t.status === "completed").length,
    speed: tasks
      .filter((t) => ACTIVE_FILTER_STATUSES.has(t.status))
      .reduce((sum, t) => sum + (speeds[t.id] || 0), 0),
  });
  const selected = $derived(tasks.find((t) => t.id === selectedId) ?? null);

  $effect(() => {
    if (deleteTarget) queueMicrotask(() => deleteCancel?.focus());
  });

  function matchesFilter(task) {
    if (filter === "all") return true;
    if (filter === "active") return ACTIVE_STATUSES.has(task.status);
    if (filter === "completed") return task.status === "completed";
    return ["paused", "failed", "canceled"].includes(task.status);
  }

  function upsert(task) {
    const i = tasks.findIndex((t) => t.id === task.id);
    if (i === -1) tasks.push(task);
    else tasks[i] = task;
  }

  function applyUpdate(task, speed) {
    const prevStatus = tasks.find((t) => t.id === task.id)?.status;
    upsert(task);
    speeds[task.id] = speed;
    if (task.status !== prevStatus && notifyOk) {
      if (task.status === "completed") {
        sendNotification({ title: "下载完成", body: task.filename });
      } else if (task.status === "failed") {
        sendNotification({ title: "下载失败", body: task.filename || task.url });
      }
    }
  }

  onMount(() => {
    let mounted = true;
    let initialTasksLoaded = false;
    const bufferedUpdates = new Map();
    const unlisteners = [];
    const closeMenu = () => (menu = null);
    const clipboardTimer = setInterval(pollClipboard, 1200);
    window.addEventListener("keydown", onKeydown);
    window.addEventListener("click", closeMenu);

    async function register(listenerPromise) {
      const unlisten = await listenerPromise;
      if (mounted) unlisteners.push(unlisten);
      else unlisten();
    }

    async function initialize() {
      try {
        await register(
          onTaskUpdate(({ task, speed }) => {
            if (initialTasksLoaded) {
              applyUpdate(task, speed);
              return;
            }
            const previous = bufferedUpdates.get(task.id);
            if (!previous || task.updated_at >= previous.task.updated_at) {
              bufferedUpdates.set(task.id, { task, speed });
            }
          }),
        );
        await register(onOpenUrl((url) => openAdd(url)));
        await register(onNewDownload(() => openAdd("")));

        const [initialTasks, loadedSettings] = await Promise.all([listTasks(), getSettings()]);
        if (!mounted) return;

        for (const { task, speed } of bufferedUpdates.values()) {
          speeds[task.id] = speed;
        }
        tasks = mergeTaskSnapshots(initialTasks, bufferedUpdates.values());
        settings = loadedSettings;
        initialTasksLoaded = true;

        notifyOk =
          (await isPermissionGranted()) || (await requestPermission()) === "granted";
      } catch (error) {
        if (mounted) quickError = String(error);
      }
    }
    initialize();

    return () => {
      mounted = false;
      unlisteners.forEach((u) => u());
      clearInterval(clipboardTimer);
      window.removeEventListener("keydown", onKeydown);
      window.removeEventListener("click", closeMenu);
    };
  });

  async function pollClipboard() {
    if (!settings?.clipboard_monitoring) return;
    let text = "";
    try {
      text = (await readText())?.trim() ?? "";
    } catch {
      return;
    }
    if (!text || text === lastClipboard || !isHttpUrl(text)) return;
    lastClipboard = text;
    quickUrl = text;
  }

  function onKeydown(event) {
    if (event.key === "Escape") {
      menu = null;
      deleteTarget = null;
      return;
    }
    const target = event.target;
    const editing =
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement;
    if (editing) return;
    if (event.ctrlKey && event.key.toLowerCase() === "n") {
      event.preventDefault();
      openAdd("");
    } else if (event.key === "Delete" && selected) {
      requestDelete(selected);
    }
  }

  function openAdd(url) {
    addUrl = url;
    addOpen = true;
  }

  function quickAdd() {
    quickError = "";
    const url = quickUrl.trim();
    if (!isHttpUrl(url)) {
      quickError = "请输入有效的 HTTP 或 HTTPS 地址。";
      return;
    }
    openAdd(url);
    quickUrl = "";
  }

  async function submitAdd(values) {
    const task = await addDownload(values);
    upsert(task);
    addOpen = false;
  }

  async function submitSettings(next) {
    const saved = await saveSettings(next);
    settings = saved;
    settingsOpen = false;
    return saved;
  }

  function requestDelete(task) {
    if (ACTIVE_STATUSES.has(task.status)) {
      quickError = "请先暂停或取消任务再删除。";
      return;
    }
    deleteTarget = task;
  }

  async function confirmDelete(withFile) {
    const task = deleteTarget;
    deleteTarget = null;
    if (!task) return;
    try {
      const ok = await deleteTask(task.id, withFile);
      if (ok) tasks = tasks.filter((t) => t.id !== task.id);
      else quickError = "请先暂停或取消任务再删除。";
    } catch (error) {
      quickError = String(error);
    }
  }

  function showMenu(event, task) {
    event.preventDefault();
    selectedId = task.id;
    menu = { task, x: event.clientX, y: event.clientY };
  }
</script>

<main>
  <header>
    <div class="brand"><span class="mark">R</span><div><div class="name">RDM</div><div class="tag">多连接下载管理器</div></div></div>
    <div class="spacer"></div>
    <button onclick={() => (settingsOpen = true)}>设置</button>
    <button class="primary" onclick={() => openAdd("")}>新建下载</button>
  </header>

  <section class="stats">
    <div class="card"><div class="k">全部任务</div><div class="v">{stats.total}</div></div>
    <div class="card"><div class="k">正在下载</div><div class="v">{stats.active}</div></div>
    <div class="card"><div class="k">已完成</div><div class="v">{stats.completed}</div></div>
    <div class="card"><div class="k">当前速度</div><div class="v">{formatSpeed(stats.speed)}</div></div>
  </section>

  <section class="quick">
    <input
      type="url"
      bind:value={quickUrl}
      placeholder="粘贴 HTTP/HTTPS 地址，按 Enter 添加"
      onkeydown={(e) => e.key === "Enter" && quickAdd()}
    />
    <button class="primary" onclick={quickAdd}>添加</button>
  </section>
  {#if quickError}<p class="banner">{quickError}</p>{/if}

  <section class="panel">
    <div class="toolbar">
      {#each [["all", "全部"], ["active", "进行中"], ["completed", "已完成"], ["other", "其他"]] as [key, label]}
        <button class="chip" class:active={filter === key} onclick={() => (filter = key)}>{label}</button>
      {/each}
      <div class="spacer"></div>
      <button disabled={!selected || ACTIVE_STATUSES.has(selected?.status) || selected?.status === "completed"} onclick={() => selected && startTask(selected.id)}>开始</button>
      <button disabled={!selected || !ACTIVE_FILTER_STATUSES.has(selected?.status)} onclick={() => selected && pauseTask(selected.id)}>暂停</button>
      <button disabled={!selected} onclick={() => selected && openFolder(selected.destination)}>目录</button>
      <button class="danger" disabled={!selected || ACTIVE_STATUSES.has(selected?.status)} onclick={() => selected && requestDelete(selected)}>删除</button>
    </div>

    <table>
      <thead>
        <tr><th>文件</th><th>大小</th><th>状态</th><th class="pcol">进度</th><th>速度</th><th>剩余</th></tr>
      </thead>
      <tbody>
        {#each visible as task (task.id)}
          <tr
            class:selected={task.id === selectedId}
            onclick={() => (selectedId = task.id)}
            ondblclick={() => openFolder(task.destination)}
            oncontextmenu={(e) => showMenu(e, task)}
          >
            <td class="name" title={task.url}>{task.filename || task.url}</td>
            <td>{task.total_size ? formatBytes(task.total_size) : "—"}</td>
            <td><span class="badge {task.status}">{statusLabel(task.status)}</span></td>
            <td class="pcol"><div class="bar"><div class="fill" style:width={`${percent(task)}%`}></div></div></td>
            <td>{ACTIVE_FILTER_STATUSES.has(task.status) ? formatSpeed(speeds[task.id]) : "—"}</td>
            <td>{formatEta(task, speeds[task.id])}</td>
          </tr>
        {:else}
          <tr><td colspan="6" class="empty">还没有下载任务。点击右上角「新建下载」开始。</td></tr>
        {/each}
      </tbody>
    </table>
  </section>
</main>

{#if menu}
  <div class="ctx" style:left={`${menu.x}px`} style:top={`${menu.y}px`}>
    {#if menu.task.status !== "completed"}
      <button onclick={() => startTask(menu.task.id)}>开始/继续</button>
    {/if}
    {#if ACTIVE_FILTER_STATUSES.has(menu.task.status)}
      <button onclick={() => pauseTask(menu.task.id)}>暂停</button>
    {/if}
    {#if menu.task.status !== "completed"}
      <button onclick={() => cancelTask(menu.task.id)}>取消</button>
    {/if}
    <button onclick={() => openFolder(menu.task.destination)}>打开所在目录</button>
    <button class="danger" onclick={() => requestDelete(menu.task)}>删除任务</button>
  </div>
{/if}

{#if addOpen && settings}
  <AddDialog {settings} initialUrl={addUrl} onsubmit={submitAdd} onclose={() => (addOpen = false)} />
{/if}

{#if settingsOpen && settings}
  <SettingsDialog {settings} onsave={submitSettings} onclose={() => (settingsOpen = false)} />
{/if}

{#if deleteTarget}
  <div
    class="overlay"
    role="presentation"
    onclick={(event) => event.target === event.currentTarget && (deleteTarget = null)}
  >
    <div class="dialog small" role="dialog" aria-modal="true" aria-labelledby="delete-title" tabindex="-1">
      <h2 id="delete-title">删除任务</h2>
      <p class="sub">是否同时删除已下载的文件？「仅删记录」会保留磁盘上的文件。</p>
      <div class="actions">
        <button bind:this={deleteCancel} class="ghost" onclick={() => (deleteTarget = null)}>取消</button>
        <button onclick={() => confirmDelete(false)}>仅删记录</button>
        <button class="danger" onclick={() => confirmDelete(true)}>删除文件</button>
      </div>
    </div>
  </div>
{/if}
