<script lang="ts">
  import { flip } from "svelte/animate";
  import Segmented from "../components/Segmented.svelte";
  import Btn from "../components/Btn.svelte";
  import Icon from "../components/Icon.svelte";
  import Dab from "../components/Dab.svelte";
  import Paint from "../components/Paint.svelte";
  import { I } from "../lib/icons";
  import { call, send } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { updateOne } from "../lib/actions";
  import { clock2, dayLabel } from "../lib/format";
  import { enter, reduced } from "../lib/motion";
  import type { ActivityEntry, ActivityKind } from "../lib/types";

  type F = "all" | "updates" | "installs" | "rollbacks" | "failures";
  let filter = $state<F>("all");
  let open = $state<string[]>([]);
  let seeded = false;

  const COLOR: Record<ActivityKind, string> = {
    update: "#7FB77E",
    install: "#7FB77E",
    uninstall: "#D9B26A",
    rollback: "#7C93D6",
    failure: "#D9706A",
    check: "#6E6E6E",
    service: "#6E6E6E",
  };
  const matches: Record<F, (e: ActivityEntry) => boolean> = {
    all: () => true,
    updates: (e) => e.kind === "update",
    installs: (e) => e.kind === "install" || e.kind === "uninstall",
    rollbacks: (e) => e.kind === "rollback",
    failures: (e) => e.kind === "failure",
  };
  const failures = $derived(store.activity.filter((e) => e.kind === "failure").length);
  const opts = $derived([
    { value: "all" as F, label: "All" },
    { value: "updates" as F, label: "Updates" },
    { value: "installs" as F, label: "Installs" },
    { value: "rollbacks" as F, label: "Rollbacks" },
    { value: "failures" as F, label: "Failures", badge: failures || "", badgeColor: "var(--err)" },
  ]);

  $effect(() => {
    // open the newest failure once so its detail is visible
    if (!seeded && store.activity.length) {
      seeded = true;
      const f = store.activity.find((e) => e.kind === "failure" && e.error);
      if (f) open = [f.id];
    }
  });

  const entries = $derived(store.activity.filter(matches[filter]).sort((a, b) => +new Date(b.time) - +new Date(a.time)));
  const days = $derived.by(() => {
    const out: { label: string; items: ActivityEntry[] }[] = [];
    for (const e of entries) {
      const label = dayLabel(e.time);
      const last = out[out.length - 1];
      if (last && last.label === label) last.items.push(e);
      else out.push({ label, items: [e] });
    }
    return out;
  });

  /** Cleanups whose saved registry entries and tasks can still be put back (30 days) */
  let restorable = $state<string[]>([]);
  let restoring = $state<string | null>(null);
  function loadBackups() {
    call("leftover_backups", {})
      .then((b) => (restorable = b.map((x) => x.id)))
      .catch(() => (restorable = []));
  }
  $effect(() => {
    // again when a cleanup adds an entry
    void store.activity.filter((e) => e.restoreId).length;
    loadBackups();
  });
  async function restore(id: string) {
    restoring = id;
    try {
      await call("leftover_restore", { backupId: id });
      store.toast("ok", "Restored");
    } catch (err) {
      store.toast("err", String((err as Error)?.message ?? err));
    }
    restoring = null;
    loadBackups();
  }

  function toggle(id: string) {
    open = open.includes(id) ? open.filter((x) => x !== id) : [...open, id];
  }
  function retry(e: ActivityEntry) {
    const j = store.failedJobs.find((x) => x.appId === e.appId);
    if (j) send("retry_job", { jobId: j.id });
    else if (e.appId) updateOne(e.appId);
  }
  function canRetry(e: ActivityEntry) {
    if (!e.appId) return false;
    const a = store.app(e.appId);
    return !!a && (a.status === "failed" || a.status === "update");
  }
  async function copy(e: ActivityEntry) {
    const text = `${e.title}\n${e.error?.code ?? ""}\n${e.error?.detail ?? ""}`.trim();
    try {
      await navigator.clipboard.writeText(text);
      store.toast("ok", "Details copied");
    } catch {
      store.toast("warn", "Could not copy. Select the text and press Ctrl C.");
    }
  }
  async function exportLog() {
    const path = await call("export_activity", {});
    if (path) store.toast("ok", `Log saved to ${path}`);
  }

  // clip-path reveal for the detail box
  function reveal(_n: Element, { duration = 200 } = {}) {
    return { duration: reduced() ? 0 : duration, css: (t: number) => `clip-path: inset(0 0 ${(1 - t) * 100}% 0); opacity: ${Math.min(1, t * 1.5)}` };
  }

  function onkey(ev: KeyboardEvent) {
    if (ev.ctrlKey && ev.key.toLowerCase() === "e") {
      ev.preventDefault();
      exportLog();
    }
  }
</script>

<svelte:window onkeydown={onkey} />

<section class="screen tight act">
  <div class="head">
    <h1 class="h1">Activity</h1>
    <div class="tools">
      <Segmented variant="chip" options={opts} value={filter} onchange={(v) => (filter = v)} label="Filter" />
      <Btn variant="ghost" size="md" title="Export log  Ctrl E" onclick={exportLog}><Icon d={I.upload} />Export log</Btn>
    </div>
  </div>

  {#if days.length === 0}
    <p class="none">{filter === "all" ? "Nothing has happened yet." : "Nothing matches this filter."}</p>
  {/if}

  <div class="days">
    {#each days as day (day.label)}
      <section aria-label={day.label} class="day">
        <h2>{day.label}</h2>
        <svg class="line" viewBox="0 0 24 400" preserveAspectRatio="none" aria-hidden="true"><path d="M9.5 3C6.8 40 7.9 92 6.6 140s-.4 104 .9 150 1.3 74 2.6 104c.3 3 2.4 4 4.6 3.2 1.8-.6 2.8-2.4 3-5 2-34 1.8-82 2.2-128s.8-96 1.4-140S21 30 19.6 8C19.4 4.4 17.4 1.6 14.6 1.3 12.2 1 9.7 1.4 9.5 3z" fill="#242424" /></svg>
        {#each day.items as e, i (e.id)}
          {@const expandable = e.kind === "failure" && !!e.error}
          {@const isOpen = open.includes(e.id)}
          <div class="ent" animate:flip={{ duration: reduced() ? 0 : 220 }} use:enter={{ i, y: 6 }}>
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <div class="line1" class:click={expandable} role={expandable ? "button" : "presentation"} tabindex={expandable ? 0 : undefined} onclick={() => expandable && toggle(e.id)} onkeydown={(k) => expandable && (k.key === "Enter" || k.key === " ") && (k.preventDefault(), toggle(e.id))}>
              <span class="tm">{clock2(e.time)}</span>
              <span class="dot"><Dab color={COLOR[e.kind]} size={10} /></span>
              <span class="tx ellipsis" style:color={e.kind === "check" || e.kind === "service" ? "var(--text-2)" : "var(--text)"}>{e.title}</span>
              {#if e.meta}<span class="mt">{e.meta}</span>{/if}
              {#if e.restoreId && restorable.includes(e.restoreId)}
                <Btn variant="raised" size="sm" title="Put back the registry entries, services and tasks this cleanup removed" disabled={restoring === e.restoreId} onclick={(ev: MouseEvent) => { ev.stopPropagation(); restore(e.restoreId!); }}>{restoring === e.restoreId ? "Restoring" : "Restore"}</Btn>
              {/if}
              {#if expandable}
                <button class="ib" aria-expanded={isOpen} aria-label={isOpen ? "Hide details" : "Show details"} title="{isOpen ? 'Hide' : 'Show'} details  Enter" onclick={(ev) => { ev.stopPropagation(); toggle(e.id); }}>
                  <span class="chev" class:up={isOpen}><Icon d={I.chevronDown} size={14} width={2.5} /></span>
                </button>
              {/if}
            </div>
            {#if expandable && isOpen && e.error}
              <div class="detail" transition:reveal>
                <pre class="selectable">{e.error.detail}</pre>
                <div class="da">
                  {#if canRetry(e)}<Btn variant="primary" size="sm" title="Retry" onclick={() => retry(e)}>Retry</Btn>{/if}
                  {#if e.error.logPath}<Btn variant="raised" size="sm" title="View log" onclick={() => send("open_log", { path: e.error!.logPath! })}>View log</Btn>{/if}
                  <Btn variant="ghost" size="sm" title="Copy details" onclick={() => copy(e)}>Copy details</Btn>
                </div>
              </div>
            {/if}
          </div>
        {/each}
      </section>
    {/each}
  </div>
</section>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .none {
    margin-top: 40px;
    color: var(--text-3);
  }
  .days {
    display: flex;
    flex-direction: column;
    gap: 20px;
    margin-top: 24px;
  }
  .day {
    position: relative;
  }
  h2 {
    height: 28px;
    display: flex;
    align-items: center;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-2);
  }
  .line {
    position: absolute;
    left: 67px;
    top: 40px;
    bottom: 14px;
    width: 4px;
    height: calc(100% - 54px);
  }
  .ent {
    display: flex;
    flex-direction: column;
  }
  .line1 {
    height: 40px;
    display: flex;
    align-items: center;
    border-radius: 6px;
  }
  .line1.click {
    cursor: pointer;
  }
  .line1.click:hover .tx {
    color: var(--text);
  }
  .tm {
    width: 64px;
    flex: none;
    font-size: 12px;
    color: var(--text-3);
  }
  .dot {
    position: relative;
    flex: none;
    margin-right: 18px;
    display: flex;
  }
  .tx {
    flex: 1;
    font-size: 13px;
  }
  .mt {
    font-size: 12px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .ib {
    width: 28px;
    height: 28px;
    margin-left: 8px;
    border-radius: 6px;
    color: var(--text-2);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast), color var(--t-fast);
  }
  .ib:hover {
    background: var(--raised);
    color: var(--text);
  }
  .chev {
    display: flex;
    transition: transform 0.13s var(--ease);
  }
  .chev.up {
    transform: rotate(180deg);
  }
  .detail {
    margin: 2px 0 10px 92px;
    padding: 16px 18px;
    border-radius: var(--r-card);
    background: #1c1c1c;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  pre {
    margin: 0;
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-2);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .da {
    display: flex;
    gap: 8px;
  }
  @media (max-width: 999px) {
    .detail {
      margin-left: 40px;
    }
    .line {
      left: 67px;
    }
  }
</style>
