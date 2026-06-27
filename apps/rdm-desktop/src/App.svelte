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
  import SniffedMediaDialog from "./components/SniffedMediaDialog.svelte";
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
    revealTaskFile,
    onTaskUpdate,
    onOpenUrl,
    onNewDownload,
    onExternalDownload,
    onSniffedMedia,
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
  let focusedTaskId = $state(null);
  let selectedTaskIds = $state(new Set());
  let quickUrl = $state("");
  let quickError = $state("");
  let page = $state("downloads");

  let addOpen = $state(false);
  let addUrl = $state("");
  let addFilename = $state("");
  let addReferrer = $state("");
  let settingsOpen = $state(false);
  let sniffedMedia = $state(null); // 浏览器扩展嗅探到的批量候选：{ candidates, pageTitle? } 或 null
  let deleteTargets = $state([]);
  let deleteCancel = $state();
  let menu = $state(null); // { task, x, y }
  let headerCheckboxEl = $state();

  let notifyOk = false;
  let lastClipboard = "";

  const navigation = [
    { id: "downloads", label: "下载中心", icon: "downloads" },
    { id: "media", label: "媒体解析", icon: "media" },
  ];
  const taskFilters = [
    { id: "all", label: "全部" },
    { id: "active", label: "进行中" },
    { id: "completed", label: "已完成" },
    { id: "other", label: "其他" },
  ];

  const sorted = $derived([...tasks].sort((a, b) => b.created_at - a.created_at));
  const visible = $derived(sorted.filter((task) => matchesTaskFilter(task, filter)));
  const stats = $derived({
    all: tasks.length,
    active: tasks.filter((t) => ACTIVE_STATUSES.has(t.status)).length,
    completed: tasks.filter((t) => t.status === "completed").length,
    other: tasks.filter((t) => ["paused", "failed", "canceled"].includes(t.status)).length,
  });
  const focusedTask = $derived(
    tasks.find((task) => task.id === focusedTaskId) ?? null,
  );
  const selectedTasks = $derived(
    tasks.filter((task) => selectedTaskIds.has(task.id)),
  );
  const selectedVisibleCount = $derived(
    visible.filter((task) => selectedTaskIds.has(task.id)).length,
  );
  const allVisibleSelected = $derived(
    visible.length > 0 && selectedVisibleCount === visible.length,
  );
  const canStartSelection = $derived(selectedTasks.some(canStartTask));
  const canPauseSelection = $derived(selectedTasks.some(canPauseTask));
  const canDeleteSelection = $derived(selectedTasks.some(canDeleteTask));

  $effect(() => {
    document.documentElement.dataset.theme = settings?.theme === "light" ? "light" : "dark";
  });

  $effect(() => {
    if (deleteTargets.length > 0) queueMicrotask(() => deleteCancel?.focus());
  });

  $effect(() => {
    if (headerCheckboxEl) {
      headerCheckboxEl.indeterminate =
        selectedVisibleCount > 0 && !allVisibleSelected;
    }
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

  function updateSelection(update) {
    const next = new Set(selectedTaskIds);
    update(next);
    selectedTaskIds = next;
  }

  function setTaskSelected(id, selected) {
    updateSelection((ids) => {
      if (selected) ids.add(id);
      else ids.delete(id);
    });
  }

  function setVisibleSelected(selected) {
    updateSelection((ids) => {
      for (const task of visible) {
        if (selected) ids.add(task.id);
        else ids.delete(task.id);
      }
    });
  }

  function clearSelection() {
    selectedTaskIds = new Set();
  }

  async function runSelectedAction(action, canRun) {
    quickError = "";
    try {
      await Promise.all(
        selectedTasks.filter(canRun).map((task) => action(task.id)),
      );
    } catch (error) {
      quickError = String(error);
    }
  }

  function requestDelete(targets) {
    const deletable = (Array.isArray(targets) ? targets : [targets]).filter(
      canDeleteTask,
    );
    if (deletable.length === 0) {
      quickError = "请先暂停或取消任务再删除。";
      return;
    }
    quickError = "";
    deleteTargets = deletable;
  }

  function removeTasks(ids) {
    tasks = tasks.filter((task) => !ids.has(task.id));
    updateSelection((selected) => {
      for (const id of ids) selected.delete(id);
    });
    if (ids.has(focusedTaskId)) focusedTaskId = null;
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
        // 浏览器扩展拦截的下载：弹出确认框，预填扩展提供的文件名。
        await register(
          onExternalDownload(({ url, filename, referrer }) => openAdd(url, filename ?? "", referrer ?? "")),
        );
        // 浏览器扩展嗅探到的一批媒体：弹出批量确认对话框。
        await register(onSniffedMedia((payload) => (sniffedMedia = payload)));

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
      deleteTargets = [];
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
    } else if (event.ctrlKey) {
      const destination = navigation[Number(event.key) - 1];
      if (!destination) return;
      event.preventDefault();
      page = destination.id;
    } else if (event.key === "Delete") {
      const targets = selectedTasks.length > 0 ? selectedTasks : focusedTask;
      if (targets) requestDelete(targets);
    }
  }

  function openAdd(url, filename = "", referrer = "") {
    page = "downloads";
    addUrl = url;
    addFilename = filename;
    addReferrer = referrer;
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
    const task = await addDownload({ ...values, referrer: addReferrer });
    upsert(task);
    addOpen = false;
    addReferrer = "";
  }

  async function downloadMedia(values) {
    upsert(await addDownload(values));
  }

  async function submitSettings(next) {
    const saved = await saveSettings(next);
    settings = saved;
    settingsOpen = false;
  }

  async function confirmDelete(withFile) {
    const targets = deleteTargets;
    deleteTargets = [];
    if (targets.length === 0) return;

    const results = await Promise.allSettled(
      targets.map((task) => deleteTask(task.id, withFile)),
    );
    const deletedIds = new Set();
    let failedCount = 0;

    results.forEach((result, index) => {
      if (result.status === "fulfilled" && result.value) {
        deletedIds.add(targets[index].id);
      } else {
        failedCount += 1;
      }
    });

    if (deletedIds.size > 0) removeTasks(deletedIds);
    if (failedCount > 0) {
      quickError = `${failedCount} 个任务删除失败或仍在进行中。`;
    }
  }

  function showMenu(event, task) {
    event.preventDefault();
    focusedTaskId = task.id;
    menu = { task, x: event.clientX, y: event.clientY };
  }
</script>

<div class="app-shell">
  <aside class="sidebar">
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
    <div class="page-content">
      {#if page === "downloads"}
        <form class="quick" onsubmit={(e) => { e.preventDefault(); quickAdd(); }}>
          <input
            type="url"
            bind:value={quickUrl}
            placeholder="粘贴文件地址，按 Enter 添加"
          />
          <button class="primary" type="submit">添加</button>
        </form>
        {#if quickError}<p class="banner">{quickError}</p>{/if}

        <section class="panel">
          <div class="panel-heading">
            <div>
              <strong>下载任务</strong>
              <span>{visible.length} 个项目</span>
            </div>
            <div class="panel-heading-actions">
              {#if selectedTasks.length > 0}
                <span class="selection-pill">已选 {selectedTasks.length} 项</span>
                <button onclick={() => runSelectedAction(startTask, canStartTask)} disabled={!canStartSelection}>开始</button>
                <button onclick={() => runSelectedAction(pauseTask, canPauseTask)} disabled={!canPauseSelection}>暂停</button>
                <button class="danger" onclick={() => requestDelete(selectedTasks)} disabled={!canDeleteSelection}>删除</button>
                <button class="ghost" onclick={clearSelection}>取消选择</button>
              {:else}
                <button disabled={!canStartTask(focusedTask)} onclick={() => focusedTask && startTask(focusedTask.id)}>开始</button>
                <button disabled={!canPauseTask(focusedTask)} onclick={() => focusedTask && pauseTask(focusedTask.id)}>暂停</button>
                <button disabled={!focusedTask} onclick={() => focusedTask && revealTaskFile(focusedTask)}><AppIcon name="folder" size={14} />定位</button>
                <button class="danger" disabled={!canDeleteTask(focusedTask)} onclick={() => focusedTask && requestDelete(focusedTask)}>删除</button>
              {/if}
            </div>
          </div>
          <div class="toolbar">
            {#each taskFilters as item}
              <button class="chip" class:active={filter === item.id} onclick={() => (filter = item.id)}>
                {item.label}<span class="chip-count">{stats[item.id]}</span>
              </button>
            {/each}
          </div>

          <div class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th class="select-cell">
                    <input
                      bind:this={headerCheckboxEl}
                      type="checkbox"
                      aria-label="全选当前筛选"
                      checked={allVisibleSelected}
                      disabled={visible.length === 0}
                      onchange={(e) => setVisibleSelected(e.currentTarget.checked)}
                    />
                  </th>
                  <th>文件</th><th>大小</th><th>状态</th><th class="pcol">进度</th><th>速度</th><th>剩余</th>
                </tr>
              </thead>
              <tbody>
                {#each visible as task (task.id)}
                  <tr
                    class:selected={task.id === focusedTaskId}
                    onclick={() => (focusedTaskId = task.id)}
                    ondblclick={() => revealTaskFile(task)}
                    oncontextmenu={(e) => showMenu(e, task)}
                  >
                    <td
                      class="select-cell"
                      onclick={(e) => e.stopPropagation()}
                      ondblclick={(e) => e.stopPropagation()}
                    >
                      <input
                        type="checkbox"
                        aria-label={`选择 ${task.filename || task.url}`}
                        checked={selectedTaskIds.has(task.id)}
                        onchange={(e) => setTaskSelected(task.id, e.currentTarget.checked)}
                      />
                    </td>
                    <td class="task-name" title={task.url}>
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
                    <td colspan="7" class="empty">
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
    <button onclick={() => revealTaskFile(menu.task)}>在文件夹中显示</button>
    {#if canDeleteTask(menu.task)}
      <button class="danger" onclick={() => requestDelete(menu.task)}>删除任务</button>
    {/if}
  </div>
{/if}

{#if addOpen && settings}
  <AddDialog
    {settings}
    initialUrl={addUrl}
    initialFilename={addFilename}
    onsubmit={submitAdd}
    onclose={() => (addOpen = false)}
  />
{/if}

{#if sniffedMedia}
  {#key sniffedMedia}
    <SniffedMediaDialog
      media={sniffedMedia}
      downloadDir={settings?.download_dir}
      onDownload={downloadMedia}
      onclose={() => (sniffedMedia = null)}
    />
  {/key}
{/if}

{#if settingsOpen && settings}
  <SettingsDialog {settings} onsave={submitSettings} onclose={() => (settingsOpen = false)} />
{/if}

{#if deleteTargets.length > 0}
  <div
    class="overlay"
    role="presentation"
    onclick={(event) => event.target === event.currentTarget && (deleteTargets = [])}
  >
    <div class="dialog small" role="dialog" aria-modal="true" aria-labelledby="delete-title" tabindex="-1">
      <h2 id="delete-title">{deleteTargets.length === 1 ? "删除任务" : `删除 ${deleteTargets.length} 个任务`}</h2>
      <p class="sub">
        是否同时删除{deleteTargets.length === 1 ? "该任务" : "这些任务"}已下载的文件？「仅删记录」会保留磁盘上的文件。
      </p>
      <div class="actions">
        <button bind:this={deleteCancel} class="ghost" onclick={() => (deleteTargets = [])}>取消</button>
        <button onclick={() => confirmDelete(false)}>仅删记录</button>
        <button class="danger" onclick={() => confirmDelete(true)}>删除文件</button>
      </div>
    </div>
  </div>
{/if}
