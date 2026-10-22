<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { flip } from "svelte/animate";
  import Splash from "../components/Splash.svelte";
  import Btn from "../components/Btn.svelte";
  import Mark from "../components/Mark.svelte";
  import Dab from "../components/Dab.svelte";
  import Icon from "../components/Icon.svelte";
  import Paint from "../components/Paint.svelte";
  import BrushBar from "../components/BrushBar.svelte";
  import Num from "../components/Num.svelte";
  import { I } from "../lib/icons";
  import { DAB } from "../lib/paths";
  import { call, send } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { router, appHref } from "../lib/router.svelte";
  import { installApp } from "../lib/actions";
  import { bytes, dateLong, preParts, splitVersion } from "../lib/format";
  import { canCancel, canPause, cancelTip, jobStatus, pauseTip, pct } from "../lib/jobs";
  import { enter, reduced, rise } from "../lib/motion";
  import type { VersionInfo } from "../lib/types";

  let { id }: { id: string } = $props();

  const FROM: Record<string, string> = { Adobe: "from Creative Cloud", "Craft apps": "from ArtCraft", Blender: "from blender.org" };

  const c = $derived(ui.catalog?.find((x) => x.id === id));
  // Only a running install of this entry shows progress, like the card in the list.
  const job = $derived.by(() => {
    const j = store.jobByApp.get(id);
    return j && j.kind === "install" ? j : undefined;
  });
  const installedApp = $derived(c?.installed ? store.app(id) : undefined);
  /** "Choose a version" entries have no concrete newest version. */
  const chooseOnly = $derived(!!c && c.pickVersion && !/^\d/.test(c.version));

  let versions = $state<VersionInfo[] | null>(null);
  let failed = $state(false);
  let seq = 0;
  function load() {
    const n = ++seq;
    failed = false;
    call("catalog_versions", { id })
      .then((r) => {
        if (n === seq) versions = r;
      })
      .catch(() => {
        if (n === seq) {
          versions = [];
          failed = true;
        }
      });
  }
  function refreshCatalog() {
    call("get_catalog", {}).then((r) => (ui.catalog = r));
  }
  onMount(() => {
    // the list from Get apps is shown at once; a direct link has to fetch it
    refreshCatalog();
    load();
    const k = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || e.ctrlKey || e.altKey || e.metaKey) return;
      const t = e.target as HTMLElement | null;
      if (t && (t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t.isContentEditable)) return;
      if (ui.popover || ui.dialog || store.blockedJobs.some((j) => !ui.dismissedBlocked.includes(j.id))) return;
      e.preventDefault();
      back();
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });
  // an install that just ended changes which versions and which entry count as installed
  let hadJob = false;
  $effect(() => {
    const has = !!job;
    if (hadJob && !has) untrack(() => (refreshCatalog(), load()));
    hadJob = has;
  });

  function back() {
    router.go("get");
  }
  function openNotes(url: string) {
    import("@tauri-apps/plugin-opener").then((m) => m.openUrl(url)).catch(() => window.open(url, "_blank"));
  }
  let list = $state<HTMLElement>();
  function chooseVersion() {
    list?.querySelector<HTMLButtonElement>("button.ins:not(:disabled)")?.focus();
    list?.scrollIntoView({ behavior: reduced() ? "auto" : "smooth", block: "start" });
  }

  const ver = $derived.by(() => {
    if (!c) return { big: "", small: "", size: 64 };
    if (chooseOnly) return { big: c.version, small: "", size: 40 };
    const [main, rest] = preParts(c.version);
    const [a, b] = splitVersion(main);
    return { big: a, small: [b, rest].filter(Boolean).join(" "), size: b || main.length > 7 ? 48 : 64 };
  });
  const installable = $derived((versions ?? []).filter((v) => !v.installed).length);
  const statusColor = $derived(job ? "var(--text)" : c?.installed ? "var(--ok)" : "var(--text-3)");
</script>

<section class="screen detail">
  <Splash colors={[c?.fg ?? "#A0A0A0"]} opacity={0.07} left lines />

  <a class="back" href="#/get" title="Back  Esc" onclick={(e) => { e.preventDefault(); back(); }}><Icon d={I.back} />Get apps</a>

  {#if !c && ui.catalog}
    <p class="gone">This app is not in the catalog any more.</p>
  {:else if !c}
    <div class="hd skhd" aria-hidden="true"><span class="sk big"></span></div>
  {:else}
    <header class="hd" in:rise>
      <Mark app={c} size={80} fs={30} />
      <div class="ttl">
        <span class="nm">
          {c.name}
          {#if c.prerelease}<span class="tag" style="color:var(--cobalt)"><Paint kind="swatch" fill="#232323" /><span class="tx">Pre-release</span></span>{/if}
        </span>
        <span class="ver" style="font-size:{ver.size}px" title={c.version}>{ver.big}{#if ver.small}<span class="vb">{ver.small.startsWith(".") ? "" : " "}{ver.small}</span>{/if}</span>
        {#if c.tagline}<span class="note">{c.tagline}</span>{/if}
      </div>
      <div class="side">
        <span class="stl" style:color={statusColor}>
          {#if job}
            <Dab color={statusColor} pulse={job.state === "downloading" || job.state === "installing"} />{jobStatus(job, false)}<span class="pc"><Num value={pct(job)} />%</span>
          {:else if c.installed}
            <Dab color={statusColor} />Installed
          {:else if c.size}
            {bytes(c.size)} download
          {/if}
        </span>
        <div class="btns">
          {#if job}
            <Btn variant="ghost" disabled={!canPause(job)} onclick={() => send(job.state === "paused" ? "resume_job" : "pause_job", { jobId: job.id })} title={pauseTip(job, "  Space")}>
              <Icon d={job.state === "paused" ? I.play : I.pause} />{job.state === "paused" ? "Resume" : "Pause"}
            </Btn>
            <Btn variant="secondary" disabled={!canCancel(job)} onclick={() => send("cancel_job", { jobId: job.id })} title={cancelTip(job, "Cancel", "  Esc")}>Cancel</Btn>
          {:else if c.installed}
            {#if installedApp}
              <Btn variant="secondary" title="Open {c.name} in Installed" onclick={() => router.go(appHref(installedApp.id))}>View in Installed</Btn>
            {/if}
          {:else if chooseOnly}
            <Btn variant="primary" title="Choose a version of {c.name}" onclick={chooseVersion}>Choose a version</Btn>
          {:else}
            <Btn variant="primary" title="Install {c.name} {c.version}" onclick={() => installApp(c)}><Icon d={I.get} size={16} width={2.5} />Install {preParts(c.version)[0]}</Btn>
          {/if}
        </div>
      </div>
    </header>

    {#if job}
      <div class="prog" in:rise><BrushBar value={job.progress} color={c.fg} height={6} /></div>
    {/if}
  {/if}

  <div class="cols">
    <section aria-label="Versions" bind:this={list}>
      <div class="sec-h">
        <span>Versions</span>
        <span class="aside">{versions && !failed ? `${installable} ${installable === 1 ? "version" : "versions"} you can install` : ""}</span>
      </div>
      {#if !versions}
        {#each [0, 1, 2, 3] as i (i)}
          <div class="vrow skel" aria-hidden="true" style="animation-delay:{i * 100}ms"><span class="sk w1"></span><span class="sk w2"></span></div>
        {/each}
      {:else if failed}
        <p class="gone">Could not load the versions. Check the connection and open this page again.</p>
      {:else}
        {#each versions as v, i (v.version)}
          {@const [vmain, vsub] = preParts(v.version)}
          <article class="vrow" class:hasnotes={v.notes.length > 0} animate:flip={{ duration: reduced() ? 0 : 200 }} use:enter={i}>
            <div class="vtop">
              <span class="vn">{vmain}{#if vsub}<span class="vs">{vsub}</span>{/if}</span>
              <span class="vd">{dateLong(v.date)}</span>
              {#if v.installed}
                <span class="tag" style="color:var(--text)"><Paint kind="swatch" fill="#232323" /><span class="tx">Installed</span></span>
              {/if}
              {#if v.latest}
                <span class="tag" style="color:var(--warn)"><Paint kind="swatch" fill="#232323" /><span class="tx">Latest</span></span>
              {/if}
              {#if v.prerelease}
                <span class="tag" style="color:var(--cobalt)"><Paint kind="swatch" fill="#232323" /><span class="tx">Pre-release</span></span>
              {/if}
              <span class="grow"></span>
              {#if v.notesUrl}
                <button class="lnk" title="Open the full release notes" onclick={() => openNotes(v.notesUrl!)}>Full notes</button>
              {/if}
              {#if c && !v.installed}
                <Btn class="ins" variant="ghost" size="sm" disabled={!!job} title="Install {c.name} {v.version}" onclick={() => installApp(c, v.version, v.size)}>Install</Btn>
              {/if}
            </div>
            {#if v.notes.length}
              <ul class="notes">
                {#each v.notes as n (n)}
                  <li><svg viewBox="0 0 20 20" width="5" height="5" aria-hidden="true"><path d={DAB} fill="#6E6E6E" /></svg>{n}</li>
                {/each}
              </ul>
            {/if}
          </article>
        {:else}
          <p class="gone">No versions are listed for this app.</p>
        {/each}
      {/if}
    </section>

    {#if c}
      <aside aria-label="Details">
        <dl>
          <dt>Source</dt>
          <dd>{c.group}<span class="mut">{FROM[c.group] ?? ""}</span></dd>
          <dt>Newest</dt>
          <dd>{chooseOnly ? "Choose a version" : c.version}</dd>
          <dt>Download</dt>
          <dd>{c.size ? bytes(c.size) : "Not known"}</dd>
          {#if c.prerelease}<dt>Release</dt><dd>Pre-release</dd>{/if}
        </dl>
        <span class="hint">
          {#if c.source === "blender"}Installs next to your other Blender versions, nothing is replaced.{:else if c.source === "adobe"}Installed through Creative Cloud. Your preferences and plugins stay as they are.{:else}Your settings stay as they are.{/if}
          {#if c.prerelease} Pre-release builds can be unstable. Do not rely on them for important work.{/if}
        </span>
      </aside>
    {/if}
  </div>
</section>

<style>
  .detail {
    padding-top: 28px;
    padding-bottom: 36px;
  }
  .back {
    position: relative;
    align-self: flex-start;
    height: 24px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    font-size: 13px;
    transition: color var(--t-fast), transform var(--t-fast) var(--ease);
  }
  .back:hover {
    color: var(--text);
    transform: translateX(-2px);
  }
  .gone {
    position: relative;
    margin-top: 16px;
    color: var(--text-3);
  }
  .hd {
    position: relative;
    display: flex;
    align-items: flex-end;
    gap: 24px;
    margin-top: 20px;
  }
  .skhd {
    height: 80px;
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .ttl {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .nm {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 15px;
    font-weight: 500;
    color: var(--text-2);
  }
  .ver {
    line-height: 0.95;
    font-weight: 600;
    letter-spacing: var(--track-display);
    white-space: nowrap;
  }
  .vb {
    font-size: 24px;
    letter-spacing: -0.02em;
    font-weight: 500;
    color: var(--text-3);
  }
  .note {
    font-size: 13px;
    color: var(--text-3);
  }
  .side {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 12px;
    flex: none;
  }
  .stl {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .pc {
    margin-left: 6px;
    font-weight: 600;
    color: var(--text);
  }
  .btns {
    display: flex;
    gap: 8px;
  }
  .prog {
    position: relative;
    margin-top: 20px;
  }
  .cols {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 288px;
    gap: 64px;
    margin-top: 36px;
    min-height: 0;
  }
  .sec-h {
    height: 28px;
  }
  .vrow {
    border-top: 1px solid var(--sep);
    padding: 8px 0;
  }
  .vrow.hasnotes {
    padding: 10px 0 14px;
  }
  .vtop {
    display: flex;
    align-items: center;
    gap: 14px;
    min-height: 28px;
  }
  .vn {
    min-width: 72px;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: var(--track-heading);
    white-space: nowrap;
  }
  .vs {
    margin-left: 8px;
    font-size: 13px;
    font-weight: 400;
    letter-spacing: 0;
    color: var(--text-3);
  }
  .vd {
    font-size: 12px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .tag {
    position: relative;
    height: 22px;
    display: inline-flex;
    align-items: center;
    padding: 0 9px;
    font-size: 12px;
    font-weight: 400;
    white-space: nowrap;
  }
  .tx {
    position: relative;
  }
  .grow {
    flex: 1;
  }
  .lnk {
    font-size: 12px;
    white-space: nowrap;
    color: var(--text-3);
    transition: color var(--t-fast);
  }
  .lnk:hover {
    color: var(--text);
  }
  .notes {
    margin: 8px 0 0 86px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .notes li {
    display: flex;
    gap: 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-2);
  }
  .notes svg {
    margin-top: 8px;
  }
  .skel {
    display: flex;
    gap: 14px;
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .sk {
    height: 20px;
    border-radius: 6px;
    background: var(--skeleton);
  }
  .sk.big {
    width: 280px;
    height: 80px;
  }
  .w1 {
    width: 72px;
  }
  .w2 {
    width: 90px;
  }
  aside {
    display: flex;
    flex-direction: column;
    gap: 20px;
    min-width: 0;
  }
  dl {
    display: grid;
    grid-template-columns: 96px minmax(0, 1fr);
    row-gap: 12px;
    column-gap: 12px;
    font-size: 13px;
  }
  dt {
    color: var(--text-3);
  }
  dd {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: flex-start;
  }
  .mut {
    font-size: 12px;
    color: var(--text-3);
  }
  .hint {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-3);
  }
  @media (max-width: 999px) {
    .cols {
      grid-template-columns: minmax(0, 1fr) 240px;
      gap: 32px;
    }
    .notes {
      margin-left: 0;
    }
  }
</style>
