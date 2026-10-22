<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "./Icon.svelte";
  import Dab from "./Dab.svelte";
  import Num from "./Num.svelte";
  import BrushBar from "./BrushBar.svelte";
  import { BRUSH } from "../lib/paths";
  import { I } from "../lib/icons";
  import { router } from "../lib/router.svelte";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { windowApi } from "../lib/api";
  import { MARKER_BASE, place } from "../lib/motion";
  import { countOf } from "../lib/format";

  const NAV = [
    { id: "home", label: "Home", href: "#/home", key: "Ctrl 1", d: I.home },
    { id: "installed", label: "Installed", href: "#/installed", key: "Ctrl 2", d: I.installed },
    { id: "get", label: "Get apps", href: "#/get", key: "Ctrl 3", d: I.get },
    { id: "queue", label: "Queue", href: "#/queue", key: "Ctrl 4", d: I.queue },
    { id: "activity", label: "Activity", href: "#/activity", key: "Ctrl 5", d: I.activity },
    { id: "services", label: "Services", href: "#/services", key: "Ctrl 6", d: I.services },
    { id: "settings", label: "Settings", href: "#/settings", key: "Ctrl ,", d: I.settings },
  ] as const;

  const active = $derived(router.route.name === "detail" ? "installed" : router.route.name === "getdetail" ? "get" : router.route.name);
  const compact = $derived(ui.compact);

  const hasJobs = $derived(store.openJobs.length > 0);
  const jobLabel = $derived.by(() => {
    const open = store.openJobs;
    if (open.every((j) => j.kind === "update")) return `Updating ${countOf(open.length, "app")}`;
    if (open.every((j) => j.kind === "install")) return `Installing ${countOf(open.length, "app")}`;
    return countOf(open.length, "job");
  });

  let nav = $state<HTMLElement>();
  let marker = $state<HTMLElement>();
  let items: Record<string, HTMLElement | undefined> = {};
  let anim = $state(false);

  function layout() {
    if (marker) place(marker, items[active], { x: 6, top: 0, bottom: 0 });
  }
  $effect(() => {
    active;
    ui.compact;
    store.updates.length;
    store.ccDown;
    tick().then(layout);
  });
  onMount(() => {
    const ro = new ResizeObserver(layout);
    if (nav) ro.observe(nav);
    for (const el of Object.values(items)) if (el) ro.observe(el);
    const id = requestAnimationFrame(() => {
      layout();
      anim = true;
    });
    document.fonts?.ready.then(layout);
    let un = () => {};
    windowApi.onResized(async () => (ui.maximized = await windowApi.isMaximized())).then((u) => (un = u));
    return () => {
      ro.disconnect();
      cancelAnimationFrame(id);
      un();
    };
  });

  function badge(id: string): { text: string; color: string } | null {
    if (id === "installed" && store.updates.length > 0) return { text: String(store.updates.length), color: "var(--warn)" };
    if (id === "services" && store.ccDown) return { text: "Off", color: "var(--err)" };
    return null;
  }
</script>

<header class="bar" data-tauri-drag-region>
  <nav aria-label="Main" bind:this={nav}>
    <span class="marker" class:anim bind:this={marker} style="width:{MARKER_BASE}px" aria-hidden="true">
      <svg viewBox="0 0 400 24" preserveAspectRatio="none"><path d={BRUSH} fill="#EDEDED" /></svg>
    </span>
    {#each NAV as n (n.id)}
      {@const b = badge(n.id)}
      <a
        bind:this={items[n.id]}
        href={n.href}
        class="item"
        class:on={active === n.id}
        class:compact
        title="{n.label}  {n.key}"
        aria-label={n.label}
        aria-current={active === n.id ? "page" : undefined}
      >
        {#if compact}<Icon d={n.d} size={18} width={2} />{/if}
        {#if !compact}<span>{n.label}</span>{/if}
        {#if b}<span class="badge" style="color:{b.color}">{b.text}</span>{/if}
      </a>
    {/each}
  </nav>

  <div class="drag" data-tauri-drag-region title="Drag to move the window"></div>

  {#if !store.status.online}
    <span class="offline"><Dab color="#D9706A" />Offline</span>
  {/if}

  {#if hasJobs}
    <a class="queue" href="#/queue" title="Open queue  Ctrl 4">
      {#if !compact}<span>{jobLabel}</span>{/if}
      <span class="qbar"><BrushBar value={store.overall} color="#EDEDED" opacity={1} height={5} track="#333333" shine={false} /></span>
      <span class="pct"><Num value={store.overall * 100} />%</span>
    </a>
  {/if}

  <button class="win" aria-label="Minimize" title="Minimize" onclick={() => windowApi.minimize()}>
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5.5h10" stroke="currentColor" stroke-width="1" /></svg>
  </button>
  <button class="win" aria-label={ui.maximized ? "Restore" : "Maximize"} title={ui.maximized ? "Restore" : "Maximize"} onclick={() => windowApi.toggleMaximize()}>
    {#if ui.maximized}
      <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="0.5" y="2.5" width="7" height="7" rx="1" fill="none" stroke="currentColor" /><path d="M2.5 2.5V1.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-1" fill="none" stroke="currentColor" /></svg>
    {:else}
      <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="0.5" y="0.5" width="9" height="9" rx="1" fill="none" stroke="currentColor" stroke-width="1" /></svg>
    {/if}
  </button>
  <button class="win close" aria-label="Close" title="Close" onclick={() => windowApi.close()}>
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0.5 0.5l9 9M9.5 0.5l-9 9" stroke="currentColor" stroke-width="1" /></svg>
  </button>
</header>

<style>
  .bar {
    position: relative;
    z-index: 80;
    height: var(--topbar-h);
    flex: none;
    display: flex;
    align-items: center;
    background: var(--bar);
    color: var(--text-2);
    padding-left: 8px;
  }
  nav {
    position: relative;
    height: 100%;
    display: flex;
    align-items: center;
  }
  .marker {
    position: absolute;
    left: 0;
    top: 0 !important;
    height: 100% !important;
    transform-origin: 0 50%;
    opacity: 0;
    pointer-events: none;
  }
  .marker svg {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 8px;
    width: 100%;
    height: 5px;
    overflow: visible;
  }
  .marker.anim {
    transition: transform 0.24s var(--ease), opacity 0.13s;
  }
  .item {
    position: relative;
    height: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 12px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-2);
    transition: color var(--t-fast) ease;
  }
  .item.compact {
    padding: 0 9px;
  }
  .item:hover {
    color: var(--text);
  }
  .item.on {
    color: var(--text);
  }
  .item:active > :global(*) {
    transform: scale(0.96);
  }
  .item > :global(*) {
    transition: transform var(--t-fast) var(--ease);
  }
  .badge {
    font-size: 12px;
    font-weight: 600;
  }
  .drag {
    flex: 1;
    align-self: stretch;
  }
  .offline {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    font-size: 12px;
    color: var(--err);
  }
  .queue {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 12px;
    margin-right: 8px;
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-2);
    font-size: 12px;
    transition: background-color var(--t-fast), transform var(--t-fast) var(--ease);
  }
  .queue:hover {
    background: var(--surface-2);
  }
  .queue:active {
    transform: scale(0.98);
  }
  .qbar {
    width: 56px;
    display: block;
  }
  .pct {
    color: var(--text);
    font-weight: 600;
    min-width: 30px;
    text-align: right;
  }
  .win {
    width: 46px;
    height: var(--topbar-h);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-2);
    transition: background-color var(--t-fast), color var(--t-fast);
  }
  .win:hover {
    background: var(--raised);
    color: var(--text);
  }
  .win.close:hover {
    color: var(--err);
  }
</style>
