<script>
  import { onMount } from "svelte";
  import packageInfo from "../package.json";
  import { readText } from "@tauri-apps/plugin-clipboard-manager";
  import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
  } from "@tauri-apps/plugin-notification";
  import AddDialog from "./components/AddDialog.svelte";
  import SettingsDialog from "./components/SettingsDialog.svelte";
  import AppIcon from "./components/AppIcon.svelte";
  import MediaResolverPage from "./components/MediaResolverPage.svelte";
  import {
    listTasks,
    getSettings,
    takeLaunchUrl,
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
  import {
    canDeleteTask,
    canPauseTask,
    canStartTask,
    matchesTaskFilter,
    mergeTaskSnapshots,
  } from "./lib/tasks.js";
  import { isHttpUrl } from "./lib/validate.js";

  let tasks = $state([]);
  let speeds = $state({});
  let settings = $state(null);
  let filter = $state("all");
  let selectedId = $state(null);
  let quickUrl = $state("");
  let quickError = $state("");
  let page = $state("downloads");

  let addOpen = $state(false);
  let addUrl = $state("");
  let settingsOpen = $state(false);
  let deleteTarget = $state(null);
  let deleteCancel = $state();
  let menu = $state(null); // { task, x, y }

  let notifyOk = false;
  let lastClipboard = "";

  const navigation = [
    { id: "downloads", label: "下载中心", icon: "downloads" },
    { id: "media", label: "媒体解析", icon: "media" },
  ];
  const pageMeta = {
    downloads: {
      title: "下载中心",
      description: "管理所有来源的下载任务",
    },
    media: {
      title: "媒体解析",
      description: "从社交媒体和网页中提取视频、音频与字幕",
    },
  };
  const currentPage = $derived(pageMeta[page]);

  const sorted = $derived([...tasks].sort((a, b) => b.created_at - a.created_at));
  const visible = $derived(sorted.filter((task) => matchesTaskFilter(task, filter)));
  const stats = $derived({
    total: tasks.length,
    active: tasks.filter((t) => ACTIVE_STATUSES.has(t.status)).length,
    completed: tasks.filter((t) => t.status === "completed").length,
    other: tasks.filter((t) => ["paused", "failed", "canceled"].includes(t.status)).length,
  });
  const selected = $derived(tasks.find((t) => t.id === selectedId) ?? null);

  $effect(() => {
    document.documentElement.dataset.theme = settings?.theme === "light" ? "light" : "dark";
  });

  $effect(() => {
    if (deleteTarget) queueMicrotask(() => deleteCancel?.focus());
  });

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

        const [initialTasks, loadedSettings, launchUrl] = await Promise.all([
          listTasks(),
          getSettings(),
          takeLaunchUrl(),
        ]);
        if (!mounted) return;

        for (const { task, speed } of bufferedUpdates.values()) {
          speeds[task.id] = speed;
        }
        tasks = mergeTaskSnapshots(initialTasks, bufferedUpdates.values());
        settings = loadedSettings;
        initialTasksLoaded = true;
        if (launchUrl) openAdd(launchUrl);

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
    } else if (event.ctrlKey && ["1", "2", "3"].includes(event.key)) {
      event.preventDefault();
      page = navigation[Number(event.key) - 1].id;
    } else if (event.key === "Delete" && selected) {
      requestDelete(selected);
    }
  }

  function openAdd(url) {
    page = "downloads";
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

  // Queue one resolved media item; reused by the media resolver page.
  async function downloadMedia(values) {
    const task = await addDownload(values);
    upsert(task);
    return task;
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

<div class="app-shell">
  <aside class="sidebar">
    <div class="brand">
      <span class="mark">R</span>
      <div><div class="brand-name">RDM</div><div class="tag">资源下载管理器</div></div>
    </div>

    <div class="nav-label">工作台</div>
    <nav aria-label="主要导航">
      {#each navigation as item}
        <button
          class="nav-item"
          class:active={page === item.id}
          onclick={() => (page = item.id)}
          title={`Ctrl+${navigation.indexOf(item) + 1}`}
        >
          <AppIcon name={item.icon} size={17} />
          <span>{item.label}</span>
          {#if item.badge}<small>{item.badge}</small>{/if}
        </button>
      {/each}
    </nav>

    <div class="sidebar-spacer"></div>
    <div class="engine-state">
      <span class="state-dot"></span>
      <div><strong>下载引擎</strong><small>本地服务运行中</small></div>
    </div>
    <button class="settings-link" onclick={() => (settingsOpen = true)}>
      <AppIcon name="settings" size={17} />
      <span>设置</span>
    </button>
    <div class="version">RDM {packageInfo.version}</div>
  </aside>

  <section class="workspace">
    <header class="topbar">
      <div>
        <h1>{currentPage.title}</h1>
        <p>{currentPage.description}</p>
      </div>
      <div class="topbar-actions">
        {#if page === "downloads"}
          <button class="primary new-download" onclick={() => openAdd("")}>
            <AppIcon name="plus" size={16} />新建下载
          </button>
        {/if}
      </div>
    </header>

    <div class="page-content">
      {#if page === "downloads"}
        <section class="quick-card">
          <div class="quick-heading">
            <span class="quick-icon"><AppIcon name="link" size={18} /></span>
            <div><strong>快速添加</strong><small>直接创建 HTTP / HTTPS 下载任务</small></div>
          </div>
          <div class="quick">
            <input
              type="url"
              bind:value={quickUrl}
              placeholder="粘贴文件地址，按 Enter 添加"
              onkeydown={(e) => e.key === "Enter" && quickAdd()}
            />
            <button class="primary" onclick={quickAdd}>添加任务</button>
          </div>
        </section>
        {#if quickError}<p class="banner">{quickError}</p>{/if}

        <section class="panel">
          <div class="panel-heading">
            <div>
              <strong>下载任务</strong>
              <span>{visible.length} 个项目</span>
            </div>
            <div class="panel-heading-actions">
              <button disabled={!canStartTask(selected)} onclick={() => selected && startTask(selected.id)}>开始</button>
              <button disabled={!canPauseTask(selected)} onclick={() => selected && pauseTask(selected.id)}>暂停</button>
              <button disabled={!selected} onclick={() => selected && openFolder(selected.destination)}><AppIcon name="folder" size={14} />目录</button>
              <button class="danger" disabled={!canDeleteTask(selected)} onclick={() => selected && requestDelete(selected)}>删除</button>
            </div>
          </div>
          <div class="toolbar">
            {#each [["all", "全部", stats.total], ["active", "进行中", stats.active], ["completed", "已完成", stats.completed], ["other", "其他", stats.other]] as [key, label, count]}
              <button class="chip" class:active={filter === key} onclick={() => (filter = key)}>
                {label}<span class="chip-count">{count}</span>
              </button>
            {/each}
          </div>

          <div class="table-wrap">
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
                    <td class="task-name" title={task.url}>
                      <span class="file-icon"><AppIcon name="downloads" size={14} /></span>
                      <span>{task.filename || task.url}</span>
                    </td>
                    <td>{task.total_size ? formatBytes(task.total_size) : "—"}</td>
                    <td><span class="badge {task.status}">{statusLabel(task.status)}</span></td>
                    <td class="pcol"><div class="bar"><div class="fill" style:width={`${percent(task)}%`}></div></div></td>
                    <td>{ACTIVE_FILTER_STATUSES.has(task.status) ? formatSpeed(speeds[task.id]) : "—"}</td>
                    <td>{formatEta(task, speeds[task.id])}</td>
                  </tr>
                {:else}
                  <tr>
                    <td colspan="6" class="empty">
                      <span class="empty-icon"><AppIcon name="downloads" size={24} /></span>
                      <strong>还没有下载任务</strong>
                      <small>添加直接链接，或从媒体解析结果中创建任务</small>
                      <button class="primary" onclick={() => openAdd("")}>新建下载</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>
      {:else if page === "media"}
        <MediaResolverPage onDownload={downloadMedia} downloadDir={settings?.download_dir} />
      {/if}
    </div>
  </section>
</div>

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
    {#if canDeleteTask(menu.task)}
      <button class="danger" onclick={() => requestDelete(menu.task)}>删除任务</button>
    {/if}
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
