<script lang="ts">
  import { Tween } from "svelte/motion";
  import { cubicOut } from "svelte/easing";
  import Paint from "./Paint.svelte";
  import Mark from "./Mark.svelte";
  import Dab from "./Dab.svelte";
  import Icon from "./Icon.svelte";
  import Btn from "./Btn.svelte";
  import Num from "./Num.svelte";
  import { WAVE_EDGE, WAVE_FILL } from "../lib/paths";
  import { I } from "../lib/icons";
  import { send } from "../lib/api";
  import { store, statusColor, statusLabel } from "../lib/store.svelte";
  import { canRepair, ui } from "../lib/ui.svelte";
  import { metaLine, openMenuFor, subColor, subline, go, retryFor } from "../lib/appview";
  import { openApp, repairApp, updateOne } from "../lib/actions";
  import { reduced } from "../lib/motion";
  import { splitVersion } from "../lib/format";
  import { canCancel, canPause, cancelTip, pauseTip } from "../lib/jobs";
  import type { AppEntry } from "../lib/types";

  let { app, compact = false }: { app: AppEntry; compact?: boolean } = $props();

  const job = $derived(store.jobByApp.get(app.id));
  const updating = $derived(app.status === "updating");
  const progress = $derived(job ? job.progress : 0);
  const level = $derived(updating ? Math.max(0.04, progress) : 0.18);
  const lv = Tween.of(() => level, { duration: reduced() ? 0 : 420, easing: cubicOut });
  const [vm, vb] = $derived(splitVersion(app.version));
  const pctVal = $derived(Math.round(progress * 100));
  let menuBtn = $state<HTMLButtonElement>();
  const menuOpen = $derived(ui.popover?.anchor === menuBtn && !!menuBtn);
  const hasJob = $derived(!!job);
  const failedJob = $derived(app.status === "failed");

  function click(e: MouseEvent) {
    if ((e.target as HTMLElement).closest("button, a")) return;
    go(app);
  }
  function key(e: KeyboardEvent) {
    if (e.target === e.currentTarget && e.key === "Enter") go(app);
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault();
      if (menuBtn) openMenuFor(app, menuBtn);
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
<article
  class="tile"
  class:compact
  aria-label={app.name}
  role="link"
  tabindex="0"
  onclick={click}
  onkeydown={key}
  oncontextmenu={(e) => {
    e.preventDefault();
    if (menuBtn) openMenuFor(app, menuBtn, { x: e.clientX, y: e.clientY });
  }}
  onpointerenter={() => (ui.focusedApp = app.id)}
  onpointerleave={() => ui.focusedApp === app.id && (ui.focusedApp = null)}
  onfocusin={() => (ui.focusedApp = app.id)}
>
  <Paint kind="blob" fill="var(--tile)" lines="M16 14c50-1.4 120-1.6 170-.4M20 186c60 1 110 .8 160-.6" lineStroke="#2A2A2A" />
  <div class="wash" aria-hidden="true">
    <div class="rise" style="transform: translateY(calc({(1 - lv.current) * 100}% - 4%))">
      <svg class="wave" class:moving={updating} viewBox="0 0 400 100" preserveAspectRatio="none">
        <path class="fillp" d={WAVE_FILL} style="fill:{app.status === 'missing' ? '#D9706A' : app.fg};fill-opacity:{updating ? 0.26 : 0.08}" />
        <path class="edge" class:on={updating} d={WAVE_EDGE} fill="none" stroke="#EDEDED" stroke-width="2" vector-effect="non-scaling-stroke" />
      </svg>
    </div>
  </div>

  <div class="body">
    <div class="head">
      <Mark app={app} size={40} fs={15} />
      <span class="status" style:color={statusColor(app.status)}>
        <Dab color={statusColor(app.status)} pulse={updating || app.status === "queued"} />{statusLabel(app.status)}
      </span>
    </div>
    <span class="name ellipsis" title={app.name}>{app.name}</span>
    {#if updating}
      <span class="big pct"><Num value={pctVal} />%</span>
    {:else}
      <span class="big" title={app.version}>{vm}{#if vb}<span class="vb">{vb}</span>{/if}</span>
    {/if}
    {#if subline(app)}<span class="subl ellipsis" style:color={updating ? "var(--text-2)" : subColor(app)}>{subline(app)}</span>{/if}
    {#if app.note && !updating}<span class="note" title={app.note}>{app.note}</span>{/if}

    <div class="foot">
      <span class="meta ellipsis">{metaLine(app)}</span>
      <span class="acts">
        {#if app.status === "update"}
          <Btn variant="raised" size="sm" title="Update to {app.latest}  U" onclick={() => updateOne(app.id)}>Update</Btn>
        {:else if app.status === "missing" && canRepair(app)}
          <Btn variant="raised" size="sm" title="Repair {app.name}" onclick={() => repairApp(app.id)}>Repair</Btn>
        {:else if failedJob}
          <Btn variant="raised" size="sm" title="Retry  R" onclick={() => retryFor(app)}>Retry</Btn>
        {:else if hasJob}
          <button class="ib" aria-label={job?.state === "paused" ? "Resume" : "Pause"} title={job ? pauseTip(job, "  Space") : ""} disabled={!job || !canPause(job)} onclick={() => job && send(job.state === "paused" ? "resume_job" : "pause_job", { jobId: job.id })}>
            <Icon d={job?.state === "paused" ? I.play : I.pause} />
          </button>
          <button class="ib" aria-label="Cancel" title={job ? cancelTip(job, "Cancel", "  Esc") : ""} disabled={!job || !canCancel(job)} onclick={() => job && send("cancel_job", { jobId: job.id })}><Icon d={I.close} /></button>
        {:else}
          <span class="hov"><Btn variant="raised" size="sm" title="Open  O" onclick={() => openApp(app.id)}>Open</Btn></span>
        {/if}
        {#if !hasJob}
          <button bind:this={menuBtn} class="ib more" class:open={menuOpen} aria-label="More actions for {app.name}" aria-haspopup="menu" aria-expanded={menuOpen} title="More  Shift F10" onclick={() => menuBtn && openMenuFor(app, menuBtn)}>
            <Icon d={I.more} width={3} />
          </button>
        {/if}
      </span>
    </div>
  </div>
</article>

<style>
  .tile {
    --tile: var(--surface);
    position: relative;
    height: 208px;
    border-radius: var(--r-card);
    overflow: hidden;
    cursor: default;
    transition: transform var(--t-base) var(--ease);
  }
  .tile.compact {
    height: 192px;
  }
  .tile:hover,
  .tile:focus-visible {
    --tile: var(--surface-2);
    transform: translateY(-2px);
  }
  .tile:focus-visible {
    outline: 1.5px solid var(--text-3);
    outline-offset: 2px;
  }
  .tile:active {
    transform: scale(0.98);
  }
  .wash {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: var(--r-card);
    pointer-events: none;
    /* Own layer: without it WebView2 can paint the wash over the whole card. */
    contain: paint;
    transform: translateZ(0);
  }
  .rise {
    position: absolute;
    left: 0;
    top: 0;
    width: 100%;
    height: 100%;
    will-change: transform;
  }
  .fillp {
    transition: fill-opacity 0.4s ease, fill 0.3s ease;
  }
  .wave {
    position: absolute;
    left: 0;
    top: 0;
    width: 200%;
    height: 100%;
  }
  .wave.moving {
    animation: wave 5.5s linear infinite;
  }
  .edge {
    opacity: 0;
    transition: opacity 0.4s ease;
  }
  .edge.on {
    opacity: 0.9;
  }
  .body {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    padding: 18px;
  }
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 20px;
    font-size: 12px;
    white-space: nowrap;
  }
  .name {
    margin-top: 16px;
    font-size: 13px;
    color: var(--text-2);
  }
  .big {
    margin-top: 6px;
    font-size: 28px;
    line-height: 1;
    font-weight: 600;
    letter-spacing: -0.03em;
    white-space: nowrap;
    min-height: 28px;
  }
  .big.pct {
    font-size: 40px;
    min-height: 28px;
    line-height: 28px;
  }
  .vb {
    font-size: 15px;
    font-weight: 500;
    letter-spacing: 0;
    color: var(--text-3);
  }
  .subl {
    margin-top: 6px;
    font-size: 12px;
  }
  .note {
    margin-top: 3px;
    font-size: 12px;
    line-height: 1.35;
    color: var(--text-3);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .foot {
    margin-top: auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    height: 28px;
  }
  .meta {
    font-size: 12px;
    color: var(--text-3);
    flex: 1;
  }
  .acts {
    display: flex;
    gap: 2px;
    flex: none;
  }
  .ib {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    color: var(--text-2);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast), color var(--t-fast), opacity var(--t-base), transform var(--t-fast) var(--ease);
  }
  .ib:hover:not(:disabled) {
    background: var(--raised);
    color: var(--text);
  }
  .ib:active:not(:disabled) {
    transform: scale(0.92);
  }
  .more,
  .hov {
    opacity: 0;
  }
  .tile:hover .more,
  .tile:focus-within .more,
  .tile:hover .hov,
  .tile:focus-within .hov,
  .more.open {
    opacity: 1;
  }
  .more.open {
    background: var(--raised-2);
    color: var(--text);
  }
  @media (hover: none) {
    .more,
    .hov {
      opacity: 1;
    }
  }
</style>
