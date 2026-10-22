<script lang="ts">
  import Mark from "./Mark.svelte";
  import Dab from "./Dab.svelte";
  import Icon from "./Icon.svelte";
  import Btn from "./Btn.svelte";
  import { I } from "../lib/icons";
  import { send } from "../lib/api";
  import { store, statusColor, statusLabel } from "../lib/store.svelte";
  import { canRepair, ui } from "../lib/ui.svelte";
  import { openMenuFor, go, retryFor } from "../lib/appview";
  import { openApp, repairApp, updateOne } from "../lib/actions";
  import { bytes, dateShort } from "../lib/format";
  import { canCancel, canPause, cancelTip, jobStatus, pauseTip } from "../lib/jobs";
  import type { AppEntry } from "../lib/types";

  let { app }: { app: AppEntry } = $props();

  const job = $derived(store.jobByApp.get(app.id));
  let menuBtn = $state<HTMLButtonElement>();
  const menuOpen = $derived(ui.popover?.anchor === menuBtn && !!menuBtn);
  const upd = $derived(app.status === "update");
  const label = $derived(app.status === "update" ? "Update available" : app.status === "current" ? "Up to date" : statusLabel(app.status));
  const latestShown = $derived(app.status === "update" ? app.latest : app.status === "missing" ? "" : (app.latest ?? app.version));

  function key(e: KeyboardEvent) {
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault();
      if (menuBtn) openMenuFor(app, menuBtn);
    }
  }
</script>

<div
  class="row"
  class:menuOpen
  role="row"
  tabindex="-1"
  onkeydown={key}
  oncontextmenu={(e) => {
    e.preventDefault();
    if (menuBtn) openMenuFor(app, menuBtn, { x: e.clientX, y: e.clientY });
  }}
  onpointerenter={() => (ui.focusedApp = app.id)}
  onpointerleave={() => ui.focusedApp === app.id && (ui.focusedApp = null)}
  onfocusin={() => (ui.focusedApp = app.id)}
>
  <span role="cell" class="c app">
    <Mark app={app} size={28} />
    <span class="nm">
      <a href="#/installed/{encodeURIComponent(app.id)}" class="ellipsis" onclick={(e) => { e.preventDefault(); go(app); }}>{app.name}</a>
      {#if app.note}<span class="n ellipsis" title={app.note}>{app.note}</span>{/if}
    </span>
  </span>
  <span role="cell" class="c ellipsis" title={app.version}>{app.version}</span>
  <span role="cell" class="c ellipsis" title={latestShown ?? ""} style:color={upd ? "var(--warn)" : "var(--text-2)"}>{latestShown}</span>
  <span role="cell" class="c st" style:color={statusColor(app.status)}>
    <Dab color={statusColor(app.status)} pulse={app.status === "updating"} />
    <span class="ellipsis">{app.status === "updating" && job ? jobStatus(job, false).replace(/^Downloading /, "") : label}</span>
  </span>
  <span role="cell" class="c right">{app.sizeOnDisk ? bytes(app.sizeOnDisk) : ""}</span>
  <span role="cell" class="c dt">{dateShort(app.updatedAt)}</span>
  <span role="cell" class="c acts">
    {#if upd}
      <Btn variant="raised" size="sm" title="Update to {app.latest}  U" onclick={() => updateOne(app.id)}>Update</Btn>
    {:else if app.status === "missing" && canRepair(app)}
      <Btn variant="raised" size="sm" onclick={() => repairApp(app.id)}>Repair</Btn>
    {:else if app.status === "failed"}
      <Btn variant="raised" size="sm" title="Retry  R" onclick={() => retryFor(app)}>Retry</Btn>
    {:else if job}
      <button class="ib" aria-label={job.state === "paused" ? "Resume" : "Pause"} title={pauseTip(job, "  Space")} disabled={!canPause(job)} onclick={() => send(job.state === "paused" ? "resume_job" : "pause_job", { jobId: job.id })}><Icon d={job.state === "paused" ? I.play : I.pause} /></button>
      <button class="ib" aria-label="Cancel" title={cancelTip(job, "Cancel", "  Esc")} disabled={!canCancel(job)} onclick={() => send("cancel_job", { jobId: job.id })}><Icon d={I.close} /></button>
    {:else}
      <Btn variant="ghost" size="sm" title="Open {app.name}  O" onclick={() => openApp(app.id)}>Open</Btn>
    {/if}
    {#if !job}
      <button bind:this={menuBtn} class="ib" class:on={menuOpen} aria-label="More actions for {app.name}" aria-haspopup="menu" aria-expanded={menuOpen} title="More  Shift F10" onclick={() => menuBtn && openMenuFor(app, menuBtn)}>
        <Icon d={I.more} width={3} />
      </button>
    {/if}
  </span>
</div>

<style>
  .row {
    position: relative;
    min-height: 52px;
    display: grid;
    grid-template-columns: var(--cols);
    align-items: center;
    border-top: 1px solid var(--sep);
    font-size: 13px;
    transition: background-color var(--t-fast) ease;
  }
  .row:hover,
  .row.menuOpen {
    background: #1b1b1b;
  }
  .c {
    min-width: 0;
    padding-right: 10px;
  }
  .app {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .nm {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .nm a {
    color: var(--text);
    font-weight: 500;
  }
  .nm a:hover {
    text-decoration: underline;
    text-decoration-color: var(--text-4);
    text-underline-offset: 3px;
  }
  .n {
    font-size: 12px;
    color: var(--text-3);
  }
  .st {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .right {
    text-align: right;
    padding-right: 16px;
    color: var(--text-2);
  }
  .dt {
    color: var(--text-2);
  }
  .acts {
    display: flex;
    justify-content: flex-end;
    gap: 4px;
    padding-right: 0;
  }
  .ib {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    color: var(--text-2);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast), color var(--t-fast), transform var(--t-fast) var(--ease);
  }
  .ib:hover:not(:disabled),
  .ib.on {
    background: var(--raised);
    color: var(--text);
  }
  .ib:active:not(:disabled) {
    transform: scale(0.92);
  }
</style>
