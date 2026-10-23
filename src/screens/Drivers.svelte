<script lang="ts">
  import { onMount } from "svelte";
  import Btn from "../components/Btn.svelte";
  import Dab from "../components/Dab.svelte";
  import Icon from "../components/Icon.svelte";
  import BrushBar from "../components/BrushBar.svelte";
  import { I } from "../lib/icons";
  import { call, send } from "../lib/api";
  import { isOpenState, statusColor, statusLabel, store } from "../lib/store.svelte";
  import { bytes, clock, countOf } from "../lib/format";
  import { jobStatus } from "../lib/jobs";
  import { enter } from "../lib/motion";
  import type { DriverEntry, Job } from "../lib/types";

  /** Opening the screen checks again when the last check is older than this. */
  const STALE_MS = 10 * 60 * 1000;

  onMount(() => {
    call("get_drivers", {}).then((d) => {
      store.drivers = d;
      const last = d.lastCheck ? Date.parse(d.lastCheck) : 0;
      if (!d.checking && Date.now() - last > STALE_MS) send("check_drivers", {});
    });
  });

  const snap = $derived(store.drivers);
  function jobFor(e: DriverEntry): Job | undefined {
    return store.jobs.find((j) => j.appId === e.id && isOpenState(j.state));
  }
  /** An installer that needs clicking through (Intel's) is open. */
  const waitsForYou = (j: Job | undefined) => !!j && j.state === "installing" && j.step.startsWith("Waiting for you");
  let confirmRestart = $state(false);
  let restarting = $state(false);
  async function restart() {
    restarting = true;
    if ((await send("restart_pc", {})) === undefined) restarting = false;
    confirmRestart = false;
  }

  function failedFor(e: DriverEntry): Job | undefined {
    return store.jobs.filter((j) => j.appId === e.id && j.state === "failed").at(-1);
  }

  const updates = $derived(snap.entries.filter((e) => e.status === "update" && !jobFor(e)));
  const updatesBytes = $derived(updates.reduce((n, e) => n + (e.size ?? 0), 0));
  const graphics = $derived(snap.entries.filter((e) => e.kind === "Graphics"));
  const devices = $derived(snap.entries.filter((e) => e.kind !== "Graphics"));
  const first = $derived(!snap.lastCheck && !snap.entries.length);

  const desc = $derived.by(() => {
    if (snap.checking) return "Asking Windows Update and your graphics card's maker. Windows Update takes a few seconds.";
    const parts: string[] = [];
    if (updates.length) parts.push(`${countOf(updates.length, "driver update")}${updatesBytes > 0 ? `, ${bytes(updatesBytes)}` : ""}.`);
    else if (snap.lastCheck) parts.push("Every driver is current.");
    if (snap.lastCheck) parts.push(`Checked at ${clock(snap.lastCheck)}.`);
    if (snap.unreachable.length) parts.push(`Could not reach ${snap.unreachable.join(" or ")}.`);
    return parts.join(" ");
  });

  function openUrl(url: string) {
    import("@tauri-apps/plugin-opener").then((o) => o.openUrl(url)).catch(() => window.open(url, "_blank"));
  }
  const SOURCE: Record<DriverEntry["source"], string> = { nvidia: "NVIDIA", amd: "AMD", intel: "Intel", windowsUpdate: "Windows Update", vendor: "Installed" };
</script>

{#snippet row(e: DriverEntry, i: number)}
  {@const job = jobFor(e)}
  {@const failed = !job ? failedFor(e) : undefined}
  {@const status = job ? (job.state === "waiting" ? "queued" : "updating") : failed && e.status === "update" ? "failed" : e.status}
  {@const waiting = waitsForYou(job)}
  <li class="row" use:enter={{ i, y: 6 }}>
    <div class="main">
      <div class="name">
        <span class="n ellipsis" title={e.name}>{e.name}</span>
        <span class="maker">{e.maker}</span>
      </div>
      <div class="meta">
        {#if e.latest && e.status === "update"}
          <span>{e.version === "None" ? "No driver yet" : e.version}</span>
          <span class="dim" aria-hidden="true">→</span>
          <span class="new">{e.latest}</span>
          {#if e.size}<span class="dim">{bytes(e.size)}</span>{/if}
        {:else}
          <span>Version {e.version}</span>
        {/if}
        <span class="dim">· {SOURCE[e.source]}</span>
      </div>
      {#if job && waiting}
        <div class="note warn">Intel's installer is open. Click through it to finish; its window may be behind this one.</div>
      {:else if job}
        <div class="job">
          <span class="jbar"><BrushBar value={job.progress} color="#7C93D6" height={5} track="#2A2A2A" shine={false} /></span>
          <span class="dim">{jobStatus(job)}</span>
        </div>
      {:else if failed && e.status === "update"}
        <div class="note err">{failed.error?.message ?? "The update failed."}</div>
      {:else if e.note}
        <div class="note">{e.note}</div>
      {/if}
    </div>
    <div class="side">
      {#if waiting}
        <span class="status" style:color="var(--warn)"><Dab color="var(--warn)" size={7} pulse />Waiting for you</span>
      {:else}
        <span class="status" style:color={statusColor(status)}><Dab color={statusColor(status)} size={7} pulse={status === "updating"} />{statusLabel(status)}</span>
      {/if}
      <div class="acts">
        {#if e.notesUrl}
          <button class="link" onclick={() => openUrl(e.notesUrl!)} title="What changed, on {e.maker}'s site">Release notes</button>
        {/if}
        {#if e.vendorUrl}
          <button class="link" onclick={() => send("open_vendor_tool", { id: e.id })} title="Newest {e.maker} drivers, from {e.maker}'s own updater">{e.maker} updater</button>
        {/if}
        {#if job}
          {#if job.state === "waiting" || job.state === "downloading" || job.state === "paused"}
            <Btn size="sm" variant="ghost" onclick={() => send("cancel_job", { jobId: job.id })}>Cancel</Btn>
          {/if}
        {:else if failed && e.status === "update"}
          <Btn size="sm" variant="raised" onclick={() => send("retry_job", { jobId: failed.id })}>Retry</Btn>
        {:else if e.status === "update"}
          <Btn size="sm" variant="raised" onclick={() => send("update_drivers", { ids: [e.id] })}>Update</Btn>
        {/if}
      </div>
    </div>
  </li>
{/snippet}

<section class="screen tight drv">
  <div class="head">
    <div class="tt">
      <h1 class="h1">Drivers</h1>
      <span class="desc">{desc}</span>
    </div>
    <div class="hb">
      <Btn variant="ghost" size="md" title="Check for driver updates" disabled={snap.checking} onclick={() => send("check_drivers", {})}>
        <span class="ck" class:spin={snap.checking}><Icon d={I.refresh} size={15} /></span>Check
      </Btn>
      {#if updates.length > 1}
        <Btn variant="primary" size="md" onclick={() => send("update_drivers", { ids: updates.map((e) => e.id) })}>Update all drivers</Btn>
      {/if}
    </div>
  </div>

  {#if snap.restartNeeded}
    <div class="restart" role="status">
      <Dab color="var(--warn)" size={8} />
      <div class="rt">
        {#if restarting}
          <strong>Restarting in a minute.</strong>
          <span>Save your work. Windows shows its own notice before it restarts.</span>
        {:else if confirmRestart}
          <strong>Restart now?</strong>
          <span>Save your work first. Your PC restarts one minute after you confirm.</span>
        {:else}
          <strong>Restart to finish installing drivers</strong>
          <span>The new drivers are installed. Some only take over after a restart.</span>
        {/if}
      </div>
      {#if !restarting}
        <div class="ra">
          {#if confirmRestart}
            <Btn variant="ghost" size="sm" onclick={() => (confirmRestart = false)}>Cancel</Btn>
            <Btn variant="primary" size="sm" onclick={restart}>Restart</Btn>
          {:else}
            <Btn variant="primary" size="sm" onclick={() => (confirmRestart = true)}>Restart now</Btn>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  {#if first || (snap.checking && !snap.entries.length)}
    <ul class="list" aria-busy="true">
      {#each [0, 1, 2] as i (i)}<li class="row skel" style="animation-delay:{i * 100}ms"></li>{/each}
    </ul>
  {:else}
    {#if graphics.length}
      <h2>Graphics</h2>
      <ul class="list">{#each graphics as e, i (e.id)}{@render row(e, i)}{/each}</ul>
    {/if}
    <h2>Other devices</h2>
    {#if devices.length}
      <ul class="list">{#each devices as e, i (e.id)}{@render row(e, i)}{/each}</ul>
    {:else}
      <p class="empty">{snap.checking ? "Checking Windows Update…" : "Windows Update has no driver updates for your other devices."}</p>
    {/if}
  {/if}

  <p class="foot">
    Graphics drivers come straight from NVIDIA, AMD and Intel, and each must be signed by its maker before it runs. Intel does not allow its graphics drivers to install
    silently, so Intel's installer opens for you to finish. Every other driver comes from Windows Update, signed by Microsoft. Your screen may blink while a graphics
    driver installs, and some drivers need a restart.
  </p>
</section>

<style>
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 24px;
  }
  .tt {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  .desc {
    max-width: 620px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-3);
  }
  .hb {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .ck {
    display: inline-flex;
    margin-right: 6px;
  }
  .spin {
    animation: spin 1s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  h2 {
    margin: 28px 0 8px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-3);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    background: var(--surface);
    border-radius: 10px;
    overflow: hidden;
  }
  .head + .list,
  .restart + .list {
    margin-top: 28px;
  }
  .restart {
    margin-top: 24px;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 16px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--warn) 10%, var(--surface));
  }
  .rt {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 13px;
  }
  .rt span {
    color: var(--text-2);
  }
  .ra {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .note.warn {
    color: var(--warn);
  }
  .row {
    display: flex;
    gap: 16px;
    padding: 14px 16px;
    border-top: 1px solid var(--sep);
  }
  .row:first-child {
    border-top: 0;
  }
  .row.skel {
    height: 66px;
    background: var(--skeleton);
  }
  .main {
    flex: 1;
    min-width: 0;
  }
  .name {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }
  .n {
    font-size: 15px;
  }
  .maker {
    flex: none;
    font-size: 12px;
    color: var(--text-3);
  }
  .meta {
    margin-top: 4px;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 13px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .new {
    color: var(--text);
  }
  .dim {
    color: var(--text-3);
  }
  .note {
    margin-top: 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .note.err {
    color: var(--err);
  }
  .job {
    margin-top: 8px;
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12px;
  }
  .jbar {
    width: 160px;
    flex: none;
  }
  .side {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    justify-content: space-between;
    gap: 8px;
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .link {
    font-size: 12px;
    color: var(--text-2);
    text-decoration: underline;
    text-decoration-color: var(--text-4);
    text-underline-offset: 3px;
  }
  .link:hover {
    color: var(--text);
  }
  .empty {
    margin: 0;
    padding: 16px;
    background: var(--surface);
    border-radius: 10px;
    color: var(--text-3);
    font-size: 13px;
  }
  .foot {
    margin: 24px 0 0;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.5;
    max-width: 640px;
  }
</style>
