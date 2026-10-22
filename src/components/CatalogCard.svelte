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
  import { router, getHref, appHref } from "../lib/router.svelte";
  import { installedFor } from "../lib/appview";
  import { ui } from "../lib/ui.svelte";
  import { installApp, openApp, updateOne } from "../lib/actions";
  import { bytes, shortVersion } from "../lib/format";
  import { blockedTitle } from "../lib/browse";
  import { canCancel, cancelTip, pct } from "../lib/jobs";
  import type { CatalogEntry } from "../lib/types";

  let { c, eager = false }: { c: CatalogEntry; eager?: boolean } = $props();

  // Only a running install of this entry shows progress; an installed app with an update is still just "Installed" here.
  const job = $derived.by(() => {
    const j = store.jobByApp.get(c.id);
    return j && j.kind === "install" ? j : undefined;
  });
  /** the installed app behind this entry (it can be a side-by-side one such as "tools:godot:4.7.2") */
  const have = $derived(installedFor(c));
  const isIn = $derived(c.installed || !!have);
  const canUpdate = $derived(have?.status === "update");
  const busy = $derived(!!job && job.state !== "waiting" && job.state !== "paused");
  const progress = $derived(job ? job.progress : 0);
  /** "Choose a version" entries have no concrete newest version, so the main button opens the list. */
  const chooseOnly = $derived(c.pickVersion && !/^\d/.test(c.version));
  const queuePos = $derived(job?.state === "waiting" ? store.waitingJobs.findIndex((j) => j.appId === c.id) + 1 : 0);

  let hover = $state(false);
  const level = $derived(job && job.state !== "waiting" ? 0.3 + 0.7 * progress : hover ? 0.5 : 0.4);
  const openId = $derived(have?.id ?? c.id);

  function open(e: MouseEvent) {
    if ((e.target as HTMLElement).closest("button, a")) return;
    router.go(getHref(c.id));
  }
  function key(e: KeyboardEvent) {
    if (e.target === e.currentTarget && e.key === "Enter") router.go(getHref(c.id));
  }

  // ----- version list -----
  const CACHE_MS = 5 * 60_000;
  function items(list: VersionInfo[]) {
    return list.map((v) => ({
      id: v.version,
      label: v.version,
      meta: [v.latest ? "Latest" : "", v.prerelease ? "Pre-release" : "", v.installed ? "Installed" : "", v.size ? bytes(v.size) : ""].filter(Boolean).join(", "),
      disabled: v.installed,
    }));
  }
  /** Pre-release builds carry long names such as "5.4.0 Alpha fbe82a4", so their list is wider. */
  const widthFor = (list: VersionInfo[]) => (list.some((v) => v.prerelease) ? 344 : 272);
  function openVersions(anchor: HTMLElement) {
    if (ui.popover?.anchor === anchor) return ui.closePopover();
    const hit = versionCache.get(c.id);
    const fresh = hit && Date.now() - hit.at < CACHE_MS;
    ui.openList({
      rect: anchor.getBoundingClientRect(),
      items: fresh ? items(hit.list) : Array.from({ length: 5 }, (_, i) => ({ id: "sk" + i, label: "", disabled: true, skeleton: true })),
      width: fresh ? widthFor(hit.list) : c.prerelease ? 344 : 272,
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
        if (ui.popover?.anchor === anchor) ui.popover = { ...ui.popover, items: items(list), width: widthFor(list) };
      })
      .catch(() => {
        if (ui.popover?.anchor === anchor) ui.closePopover();
        store.toast("err", `Could not load the versions of ${c.name}`);
      });
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
<article
  class="card"
  aria-label={c.name}
  role="link"
  tabindex="0"
  onclick={open}
  onkeydown={key}
  onpointerenter={() => (hover = true)}
  onpointerleave={() => (hover = false)}
>
  <Paint kind="blob" fill="var(--card)" />
  <Wash color={c.fg} level={level} active={busy} rest={c.installed && !job ? 0.07 : 0.12} />
  <div class="in">
    <div class="top">
      <Mark app={c} size={48} fs={17} {eager} />
      <div class="nm">
        <span class="n"><span class="nt ellipsis" title={c.name}>{c.name}</span>{#if c.prerelease}<span class="beta">Pre-release</span>{/if}</span>
        <span class="s ellipsis" title={c.version}>{shortVersion(c.version)}{c.size ? `, ${bytes(c.size)}` : ""}</span>
      </div>
    </div>
    <span class="tag ellipsis" title={c.tagline ?? ""}>{c.tagline ?? ""}</span>
    <div class="bot">
      <span class="note">
        {#if job}
          {job.state === "waiting" ? `Queued, ${queuePos}${["st", "nd", "rd"][queuePos - 1] ?? "th"}` : job.state === "paused" ? "Paused" : job.ready ? "Downloaded, ready to install" : `Installing, ${pct(job)}%`}
        {:else if isIn}
          {#if have}
            <a class="ok inst" href={appHref(have.id)} title="Installed {have.version}. Show in Installed" onclick={(e) => { e.preventDefault(); router.go(appHref(have.id)); }}>
              <Dab color="var(--ok)" /><span class="ellipsis">{canUpdate ? have.version : `Installed ${have.version}`}</span>
            </a>
          {:else}
            <span class="ok"><Dab color="var(--ok)" />Installed</span>
          {/if}
        {:else if c.blocked}
          <span class="blk" title={blockedTitle(c.blocked)}>Can't be installed silently</span>
        {/if}
      </span>
      {#if job}
        <button class="ib" aria-label="Cancel install of {c.name}" title={cancelTip(job)} disabled={!canCancel(job)} onclick={() => send("cancel_job", { jobId: job.id })}><Icon d={I.close} /></button>
      {:else if isIn}
        <div class="acts">
          {#if canUpdate && have}
            <Btn variant="raised" size="sm" title="Update {c.name} to {have.latest}" onclick={() => updateOne(have.id)}>Update</Btn>
            <button class="ib" aria-label="Open {c.name}" title="Open {c.name}" onclick={() => openApp(openId)}><Icon d={I.external} size={14} width={2.5} /></button>
          {:else}
            <Btn variant="raised" size="sm" title="Open {c.name}" onclick={() => openApp(openId)}>Open<Icon d={I.external} size={13} width={2.5} /></Btn>
          {/if}
        </div>
      {:else if c.blocked}
        <Btn variant="raised" size="sm" disabled title={blockedTitle(c.blocked)}><Icon d={I.get} size={14} width={2.5} />Install</Btn>
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
    min-width: 0;
    font-size: 12px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .blk {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-3);
  }
  .acts {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .ok.inst {
    max-width: 100%;
    min-width: 0;
    transition: color var(--t-fast);
  }
  .ok.inst:hover {
    color: var(--text);
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
  .ib:hover:not(:disabled) {
    background: var(--raised);
    color: var(--text);
  }
</style>
