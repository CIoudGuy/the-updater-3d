<script lang="ts">
  import { flip } from "svelte/animate";
  import Btn from "../components/Btn.svelte";
  import Mark from "../components/Mark.svelte";
  import Icon from "../components/Icon.svelte";
  import Num from "../components/Num.svelte";
  import Paint from "../components/Paint.svelte";
  import Select from "../components/Select.svelte";
  import BrushBar from "../components/BrushBar.svelte";
  import Dab from "../components/Dab.svelte";
  import { BRUSH } from "../lib/paths";
  import { I } from "../lib/icons";
  import { send } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { isPaused, togglePauseAll } from "../lib/actions";
  import { approx, bytes, clock, countOf, inUnit, unitFor } from "../lib/format";
  import { NO_STOP_TIP, canCancel, canPause, cancelTip, jobStatus, jobWhat, pauseTip, pct } from "../lib/jobs";
  import { enter, reduced } from "../lib/motion";
  import type { Job } from "../lib/types";
  import { jobHref } from "../lib/links";

  const s = $derived(store.settings);
  const limitMb = $derived(s.bandwidthLimitEnabled && s.bandwidthLimit > 0 ? Math.round(s.bandwidthLimit / 1e6) : 0);
  const bwOpts = $derived.by(() => {
    const base = [0, 5, 10, 20, 40, 80, 150];
    if (!base.includes(limitMb)) base.push(limitMb);
    return base.sort((a, b) => a - b).map((n) => ({ value: String(n), label: n === 0 ? "Unlimited" : `${n} MB/s` }));
  });
  function setLimit(v: string) {
    const n = Number(v);
    if (n === 0) store.patchSettings({ bandwidthLimitEnabled: false });
    else store.patchSettings({ bandwidthLimit: n * 1e6, bandwidthLimitEnabled: true });
  }

  const active = $derived(store.activeJobs);
  const paused = $derived(isPaused());
  const dlU = $derived(unitFor(store.totalBytes || 1));

  // ----- reorder waiting jobs by dragging -----
  const H = 56;
  let localOrder = $state<string[] | null>(null);
  const waiting = $derived.by(() => {
    const base = store.waitingJobs;
    if (!localOrder) return base;
    const map = new Map(base.map((j) => [j.id, j]));
    const ordered = localOrder.map((id) => map.get(id)).filter((j): j is Job => !!j);
    for (const j of base) if (!localOrder.includes(j.id)) ordered.push(j);
    return ordered;
  });
  let drag = $state<{ id: string; from: number; to: number; dy: number; startY: number; settling: boolean } | null>(null);

  function commit(ids: string[]) {
    localOrder = ids;
    send("reorder_jobs", { jobIds: ids });
    setTimeout(() => (localOrder = null), 700);
  }
  function down(e: PointerEvent, id: string, index: number) {
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { id, from: index, to: index, dy: 0, startY: e.clientY, settling: false };
  }
  function move(e: PointerEvent) {
    if (!drag || drag.settling) return;
    const dy = e.clientY - drag.startY;
    const to = Math.max(0, Math.min(waiting.length - 1, Math.round(drag.from + dy / H)));
    drag = { ...drag, dy, to };
  }
  function up() {
    if (!drag || drag.settling) return;
    const d = drag;
    drag = { ...d, dy: (d.to - d.from) * H, settling: true };
    setTimeout(() => {
      if (d.to !== d.from) {
        const ids = waiting.map((j) => j.id);
        const [m] = ids.splice(d.from, 1);
        ids.splice(d.to, 0, m);
        commit(ids);
      }
      drag = null;
    }, reduced() ? 0 : 150);
  }
  function gripKey(e: KeyboardEvent, index: number) {
    const dir = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0;
    if (!dir) return;
    e.preventDefault();
    const to = index + dir;
    if (to < 0 || to >= waiting.length) return;
    const ids = waiting.map((j) => j.id);
    const [m] = ids.splice(index, 1);
    ids.splice(to, 0, m);
    commit(ids);
  }
  function shift(i: number): string {
    if (!drag) return "none";
    if (i === drag.from) return `translate(${drag.settling ? 0 : 8}px, ${drag.dy}px)`;
    if (drag.from < drag.to && i > drag.from && i <= drag.to) return `translateY(${-H}px)`;
    if (drag.from > drag.to && i >= drag.to && i < drag.from) return `translateY(${H}px)`;
    return "none";
  }
  const ord = (n: number) => `${n}${["th", "st", "nd", "rd"][n % 100 > 10 && n % 100 < 14 ? 0 : n % 10 < 4 ? n % 10 : 0]}`;

  function statusColor(j: Job) {
    return j.state === "paused" || j.state === "blocked" ? "var(--warn)" : "var(--text-2)";
  }
</script>

<section class="screen tight queue">
  <div class="head">
    <div class="tt">
      <h1 class="h1">Queue</h1>
      <span class="sub">{active.length} active, {waiting.length} waiting.{waiting.length > 1 ? " Drag to change the order." : ""}</span>
    </div>
    <div class="ctl">
      <label for="bw" class="lbl">Bandwidth limit</label>
      <Select id="bw" options={bwOpts} value={String(limitMb)} onchange={setLimit} width={130} label="Bandwidth limit" title="Bandwidth limit  Ctrl Shift L" />
      <Btn variant="secondary" size="md" hint="Ctrl P" title={!store.openJobs.some(canPause) && store.openJobs.length > 0 ? NO_STOP_TIP : `${paused ? "Resume all" : "Pause all"}  Ctrl P`} onclick={togglePauseAll} disabled={!store.openJobs.some(canPause)}>{paused ? "Resume all" : "Pause all"}</Btn>
    </div>
  </div>

  {#if store.openJobs.length > 0}
  <div class="stats">
    <div class="stat">
      <span class="k">Speed</span>
      <span class="v"><Num value={store.speed / 1e6} fmt={(n) => String(Math.round(n))} /> <span class="u">{limitMb ? `of ${limitMb} ` : ""}MB/s</span></span>
    </div>
    <div class="stat">
      <span class="k">Downloaded</span>
      <span class="v"><Num value={inUnit(store.downloaded, dlU)} fmt={(n) => n.toFixed(1)} /> <span class="u">of {bytes(store.totalBytes)}</span></span>
    </div>
    <div class="stat">
      <span class="k">Time left</span>
      <span class="v">{store.eta == null ? "None" : approx(store.eta)}</span>
    </div>
  </div>
  {/if}

  {#if store.openJobs.length === 0}
    <p class="empty">Nothing in the queue. Updates and installs you start show here.</p>
  {/if}

  {#if active.length}
    <section aria-label="Active" class="sec">
      <div class="sec-h"><span>Active</span></div>
      {#each active as j, i (j.id)}
        <div class="arow" animate:flip={{ duration: reduced() ? 0 : 140 }} use:enter={i}>
          <svg class="grip dim" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="#4a4a4a" stroke-width="3" stroke-linecap="round" aria-hidden="true"><path d={I.grip} /></svg>
          <svelte:element this={jobHref(j) ? "a" : "div"} class="who" href={jobHref(j)} title={jobHref(j) ? `Open ${j.name}` : undefined}>
            <Mark app={j} size={32} />
            <div class="nm"><span class="n">{j.name}</span><span class="w">{jobWhat(j)}</span></div>
          </svelte:element>
          <span class="st ellipsis" style:color={statusColor(j)}>{jobStatus(j, false)}</span>
          <span class="sp">{j.state === "downloading" && j.speed > 0 ? `${Math.round(j.speed / 1e6)} MB/s` : ""}</span>
          <span class="et">{j.eta != null && j.state !== "paused" && j.state !== "blocked" ? approx(j.eta).replace("About ", "") : ""}</span>
          <span class="pc" style:color={j.state === "paused" ? "var(--text-3)" : "var(--text)"}><Num value={pct(j)} />%</span>
          <span class="bt">
            <button class="ib" aria-label="{j.state === 'paused' ? 'Resume' : 'Pause'} {j.name}" title={pauseTip(j, "  Space")} disabled={!canPause(j)} onclick={() => send(j.state === "paused" ? "resume_job" : "pause_job", { jobId: j.id })}><Icon d={j.state === "paused" ? I.play : I.pause} /></button>
            <button class="ib" aria-label="Cancel {j.name}" title={cancelTip(j, "Cancel", "  Del")} disabled={!canCancel(j)} onclick={() => send("cancel_job", { jobId: j.id })}><Icon d={I.close} /></button>
          </span>
          <span class="bar"><BrushBar value={j.progress} color={j.fg} height={6} fillOpacityOverride={j.state === "paused" ? 0.4 : 0.85} /></span>
        </div>
      {/each}
    </section>
  {/if}

  {#if waiting.length}
    <section aria-label="Waiting" class="sec">
      <div class="sec-h"><span>Waiting</span></div>
      <div class="wline" aria-hidden="true"><svg viewBox="0 0 400 24" preserveAspectRatio="none"><path d={BRUSH} fill="#EDEDED" fill-opacity="0.8" /></svg></div>
      <div class="wlist" role="list">
        {#each waiting as w, i (w.id)}
          {@const dragging = drag?.id === w.id}
          <div
            class="wrow"
            class:dragging
            class:settling={drag?.settling}
            class:other={!!drag && !dragging}
            role="listitem"
            style="transform:{shift(i)}"
            use:enter={i}
          >
            {#if dragging}<span class="dragbg"><Paint kind="swatch" fill="#262626" /></span>{/if}
            <button
              class="grip"
              class:on={dragging}
              aria-label="Drag to reorder {w.name}. Use arrow keys to move."
              title="Drag to reorder"
              onpointerdown={(e) => down(e, w.id, i)}
              onpointermove={move}
              onpointerup={up}
              onpointercancel={up}
              onkeydown={(e) => gripKey(e, i)}
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" aria-hidden="true"><path d={I.grip} /></svg>
            </button>
            <svelte:element this={jobHref(w) ? "a" : "div"} class="who" href={jobHref(w)} title={jobHref(w) ? `Open ${w.name}` : undefined}>
              <Mark app={w} size={32} />
              <div class="nm"><span class="n">{w.name}</span><span class="w">{jobWhat(w)}</span></div>
            </svelte:element>
            <span class="st mut ellipsis">{dragging ? `Moving to ${ord((drag?.to ?? 0) + 1)}` : `Waiting${i === 0 ? ", next" : ""}`}</span>
            <button class="ib" aria-label="Cancel {w.name}" title="Cancel  Del" onclick={() => send("cancel_job", { jobId: w.id })}><Icon d={I.close} /></button>
          </div>
        {/each}
      </div>
    </section>
  {/if}

  {#if store.finishedJobs.length}
    <section aria-label="Finished" class="sec">
      <div class="sec-h">
        <span>Finished</span>
        <Btn variant="ghost" size="sm" title="Clear finished jobs" onclick={() => send("clear_finished_jobs", {})}>Clear</Btn>
      </div>
      {#each store.finishedJobs as j, i (j.id)}
        <div class="frow" animate:flip={{ duration: reduced() ? 0 : 140 }} use:enter={i}>
          <svelte:element this={jobHref(j) ? "a" : "div"} class="who" href={jobHref(j)} title={jobHref(j) ? `Open ${j.name}` : undefined}>
            <Mark app={j} size={28} />
            <div class="nm"><span class="n">{j.name}</span><span class="w">{jobWhat(j)}</span></div>
          </svelte:element>
          <span class="st fin ellipsis" style:color={j.state === "failed" ? "var(--err)" : j.state === "done" ? "var(--ok)" : "var(--text-3)"}>
            <Dab color={j.state === "failed" ? "#D9706A" : j.state === "done" ? "#7FB77E" : "#6E6E6E"} />
            {j.state === "done" ? "Finished" : j.state === "failed" ? (j.error ? `Failed, ${j.error.code}` : "Failed") : "Canceled"}
          </span>
          <span class="tm">{j.finishedAt ? clock(j.finishedAt) : ""}</span>
          {#if j.state === "failed"}
            <Btn variant="raised" size="sm" title="Retry  R" onclick={() => send("retry_job", { jobId: j.id })}>Retry</Btn>
          {/if}
        </div>
      {/each}
    </section>
  {/if}
</section>

<style>
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
  }
  .tt {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .ctl {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .lbl {
    font-size: 12px;
    color: var(--text-3);
  }
  .stats {
    margin-top: 28px;
  }
  .empty {
    margin-top: 40px;
    color: var(--text-3);
    font-size: 13px;
  }
  .sec {
    margin-top: 28px;
    position: relative;
  }
  .sec-h {
    height: 28px;
  }
  .arow {
    position: relative;
    height: 64px;
    display: flex;
    align-items: center;
    gap: 14px;
    border-top: 1px solid var(--sep);
  }
  .who {
    display: flex;
    align-items: center;
    gap: inherit;
    flex: none;
    color: inherit;
    text-decoration: none;
    border-radius: 6px;
  }
  a.who .n {
    transition: color var(--t-fast) ease;
  }
  a.who:hover .n,
  a.who:focus-visible .n {
    color: var(--accent, #fff);
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-thickness: 1px;
  }
  .nm {
    width: 200px;
    flex: none;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .n {
    font-size: 13px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .w {
    font-size: 12px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .st {
    flex: 1;
    min-width: 0;
    font-size: 12px;
  }
  .st.mut {
    color: var(--text-3);
  }
  .st.fin {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .sp {
    width: 72px;
    font-size: 12px;
    color: var(--text-2);
  }
  .et {
    width: 80px;
    font-size: 12px;
    color: var(--text-2);
  }
  .pc {
    width: 56px;
    text-align: right;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .bt {
    display: flex;
    gap: 2px;
  }
  .bar {
    position: absolute;
    left: 76px;
    right: 0;
    bottom: 5px;
  }
  .grip {
    position: relative;
    width: 16px;
    height: 28px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #6e6e6e;
    touch-action: none;
    cursor: grab;
    border-radius: 4px;
    transition: color var(--t-fast);
  }
  .grip:hover,
  .grip.on {
    color: var(--text);
  }
  .grip.on {
    cursor: grabbing;
  }
  .grip.dim {
    height: 16px;
  }
  .wline {
    position: relative;
    height: 0;
    border-top: 1px solid var(--sep);
  }
  .wline svg {
    position: absolute;
    left: 0;
    right: 0;
    top: -4px;
    width: 100%;
    height: 6px;
  }
  .wlist {
    position: relative;
  }
  .wrow {
    position: relative;
    height: 56px;
    display: flex;
    align-items: center;
    gap: 14px;
    border-top: 1px solid var(--sep);
  }
  .wrow:first-child {
    border-top-color: transparent;
  }
  .wrow.other,
  .wrow.settling {
    transition: transform 0.1s var(--ease);
  }
  .wrow.dragging {
    z-index: 3;
    padding: 0 12px;
    margin: 0 -12px;
    border-top-color: transparent;
  }
  .wrow.dragging.settling {
    transition: transform 0.1s var(--ease);
  }
  .dragbg {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .wrow > :global(*:not(.dragbg)) {
    position: relative;
  }
  .ib {
    width: 28px;
    height: 28px;
    flex: none;
    border-radius: 6px;
    color: var(--text-2);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast), color var(--t-fast), transform var(--t-fast) var(--ease);
  }
  .ib:hover:not(:disabled) {
    background: var(--raised);
    color: var(--text);
  }
  .ib:active:not(:disabled) {
    transform: scale(0.92);
  }
  .frow {
    min-height: 52px;
    display: flex;
    align-items: center;
    gap: 14px;
    border-top: 1px solid var(--sep);
  }
  .tm {
    font-size: 12px;
    color: var(--text-3);
    min-width: 40px;
    text-align: right;
  }
  @media (max-width: 999px) {
    .nm {
      width: 150px;
    }
    .sp {
      display: none;
    }
    .et {
      width: 60px;
    }
  }
</style>
