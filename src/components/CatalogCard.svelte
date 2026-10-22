<script lang="ts" module>
  import type { VersionInfo } from "../lib/types";
  // version lists are shared between all cards for the session
  const versionCache = new Map<string, { at: number; list: VersionInfo[] }>();
</script>

<script lang="ts">
  import Paint from "./Paint.svelte";
  import Mark from "./Mark.svelte";
  import Icon from "./Icon.svelte";
  import Dab from "./Dab.svelte";
  import Btn from "./Btn.svelte";
  import Wash from "./Wash.svelte";
  import SplitBtn from "./SplitBtn.svelte";
  import { I } from "../lib/icons";
  import { call, send } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { installApp, openApp } from "../lib/actions";
  import { bytes } from "../lib/format";
  import { pct } from "../lib/jobs";
  import type { CatalogEntry } from "../lib/types";

  let { c, eager = false }: { c: CatalogEntry; eager?: boolean } = $props();

  // Only a running install of this entry shows progress; an installed app with an update is still just "Installed" here.
  const job = $derived.by(() => {
    const j = store.jobByApp.get(c.id);
    return j && j.kind === "install" ? j : undefined;
  });
  const busy = $derived(!!job && job.state !== "waiting" && job.state !== "paused");
  const progress = $derived(job ? job.progress : 0);
  /** "Choose a version" entries have no concrete newest version, so the main button opens the list. */
  const chooseOnly = $derived(c.pickVersion && !/^\d/.test(c.version));
  const queuePos = $derived(job?.state === "waiting" ? store.waitingJobs.findIndex((j) => j.appId === c.id) + 1 : 0);

  let hover = $state(false);
  const level = $derived(job && job.state !== "waiting" ? 0.3 + 0.7 * progress : hover ? 0.5 : 0.4);

  // ----- version list -----
  const CACHE_MS = 5 * 60_000;
  function items(list: VersionInfo[]) {
    return list.map((v) => ({
      id: v.version,
      label: v.version,
      meta: [v.latest ? "Latest" : "", v.prerelease ? "Beta" : "", v.installed ? "Installed" : "", v.size ? bytes(v.size) : ""].filter(Boolean).join(", "),
      disabled: v.installed,
    }));
  }
  function openVersions(anchor: HTMLElement) {
    if (ui.popover?.anchor === anchor) return ui.closePopover();
    const hit = versionCache.get(c.id);
    const fresh = hit && Date.now() - hit.at < CACHE_MS;
    ui.openList({
      rect: anchor.getBoundingClientRect(),
      items: fresh ? items(hit.list) : Array.from({ length: 5 }, (_, i) => ({ id: "sk" + i, label: "", disabled: true, skeleton: true })),
      width: 272,
      align: "end",
      label: `Versions of ${c.name}`,
      anchor,
      onpick: (v) => installApp(c, v),
    });
    if (fresh) return;
    call("catalog_versions", { id: c.id })
      .then((list) => {
        versionCache.set(c.id, { at: Date.now(), list });
        // the menu may have been closed or moved to another card while the list loaded
        if (ui.popover?.anchor === anchor) ui.popover = { ...ui.popover, items: items(list) };
      })
      .catch(() => {
        if (ui.popover?.anchor === anchor) ui.closePopover();
        store.toast("err", `Could not load the versions of ${c.name}`);
      });
  }
</script>

<article
  class="card"
  aria-label={c.name}
  onpointerenter={() => (hover = true)}
  onpointerleave={() => (hover = false)}
>
  <Paint kind="blob" fill="var(--card)" />
  <Wash color={c.fg} level={level} active={busy} rest={c.installed && !job ? 0.07 : 0.12} />
  <div class="in">
    <div class="top">
      <Mark app={c} size={48} fs={17} {eager} />
      <div class="nm">
        <span class="n"><span class="nt ellipsis" title={c.name}>{c.name}</span>{#if c.prerelease}<span class="beta">Beta</span>{/if}</span>
        <span class="s ellipsis">{c.version}{c.size ? `, ${bytes(c.size)}` : ""}</span>
      </div>
    </div>
    <span class="tag ellipsis" title={c.tagline ?? ""}>{c.tagline ?? ""}</span>
    <div class="bot">
      <span class="note">
        {#if job}
          {job.state === "waiting" ? `Queued, ${queuePos}${["st", "nd", "rd"][queuePos - 1] ?? "th"}` : job.state === "paused" ? "Paused" : `Installing, ${pct(job)}%`}
        {:else if c.installed}
          <span class="ok"><Dab color="var(--ok)" />Installed</span>
        {/if}
      </span>
      {#if job}
        <button class="ib" aria-label="Cancel install of {c.name}" title="Cancel" onclick={() => send("cancel_job", { jobId: job.id })}><Icon d={I.close} /></button>
      {:else if c.installed}
        <Btn variant="raised" size="sm" title="Open {c.name}" onclick={() => openApp(c.id)}>Open<Icon d={I.external} size={13} width={2.5} /></Btn>
      {:else if c.pickVersion}
        <SplitBtn
          title={chooseOnly ? `Choose a version of ${c.name}` : `Install ${c.name} ${c.version}`}
          menuLabel="Choose a version of {c.name}"
          onmain={() => (chooseOnly ? undefined : installApp(c))}
          onmenu={openVersions}
        >
          {#if chooseOnly}Choose version{:else}<Icon d={I.get} size={14} width={2.5} />Install{/if}
        </SplitBtn>
      {:else}
        <Btn variant="raised" size="sm" title="Install {c.name} {c.version}" onclick={() => installApp(c)}><Icon d={I.get} size={14} width={2.5} />Install</Btn>
      {/if}
    </div>
  </div>
</article>

<style>
  .card {
    --card: var(--surface);
    position: relative;
    height: 168px;
    border-radius: var(--r-card);
    overflow: hidden;
    transition: transform var(--t-base) var(--ease);
  }
  .card:hover,
  .card:focus-within {
    --card: var(--surface-2);
    transform: translateY(-2px);
  }
  .in {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    padding: 16px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .nm {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .n {
    display: flex;
    align-items: baseline;
    min-width: 0;
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .beta {
    flex: none;
    margin-left: 8px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--raised);
    font-size: 11px;
    font-weight: 500;
    letter-spacing: 0;
    color: var(--cobalt);
  }
  .s {
    font-size: 12px;
    color: var(--text-3);
  }
  .tag {
    display: block;
    margin-top: 12px;
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-2);
  }
  .bot {
    margin-top: auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    height: 28px;
  }
  .note {
    font-size: 12px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .ok {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--ok);
    font-weight: 500;
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
  .ib:hover {
    background: var(--raised);
    color: var(--text);
  }
</style>
