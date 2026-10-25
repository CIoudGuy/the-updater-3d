<script lang="ts">
  import { onMount } from "svelte";
  import { flip } from "svelte/animate";
  import Splash from "../components/Splash.svelte";
  import NotesList from "../components/NotesList.svelte";
  import Btn from "../components/Btn.svelte";
  import Mark from "../components/Mark.svelte";
  import Dab from "../components/Dab.svelte";
  import Icon from "../components/Icon.svelte";
  import Paint from "../components/Paint.svelte";
  import Select from "../components/Select.svelte";
  import Segmented from "../components/Segmented.svelte";
  import Toggle from "../components/Toggle.svelte";
  import BrushBar from "../components/BrushBar.svelte";
  import Num from "../components/Num.svelte";
  import OtherBuilds from "../components/OtherBuilds.svelte";
  import { I } from "../lib/icons";
  import { DAB } from "../lib/paths";
  import { call, on, send } from "../lib/api";
  import { store, statusColor } from "../lib/store.svelte";
  import { canRepair, isIgnored, ui } from "../lib/ui.svelte";
  import { router } from "../lib/router.svelte";
  import { askUninstall, installVersion, modalOpen, openApp, openStorePage, repairApp, setIgnored, showInFolder, updateOne } from "../lib/actions";
  import { tryLoad } from "../lib/load";
  import { aboutLine, listedOnly } from "../lib/view";
  import { bytes, cmpVersion, dateLong, dateShort, preParts, splitVersion } from "../lib/format";
  import { canCancel, canPause, cancelTip, jobStatus, pauseTip, pct } from "../lib/jobs";
  import { enter, reduced, rise } from "../lib/motion";
  import type { VersionInfo } from "../lib/types";

  let { id }: { id: string } = $props();

  const app = $derived(store.app(id));
  // This app's own entry: its page is the app's own page, with every release.
  $effect(() => {
    if (app?.isSelf) router.replace("updater");
  });
  const job = $derived(app ? store.jobByApp.get(app.id) : undefined);
  const isBlender = $derived(app?.source === "blender");
  const isAdobe = $derived(app?.source === "adobe");
  /** a device driver, the chipset or the BIOS: installs only its newest version, never rolls back */
  const isDriver = $derived(app?.source === "drivers");
  /** its updates are turned off ("Don't update") */
  const ignored = $derived(!!app && isIgnored(app.id));
  /** FreeCAD, Godot and Unity install each version next to the others (ids "tools:godot:4.7.2"); OBS Studio and Krita update in place. */
  const sideBySide = $derived(isBlender || (app?.source === "tools" && app.id.split(":").length > 2));

  let versions = $state<VersionInfo[] | null>(null);
  /** the version list could not be loaded (offline); shown with a Retry button */
  let versionsError = $state<string | null>(null);
  /** Stable or pre-release updates: shown when the app has pre-releases to choose (an Other app's show in its version list) */
  const preOn = $derived(app?.preChannel ?? null);
  const preShown = $derived(!!app && !app.readOnly && !app.isSelf && (preOn !== null || (app.source === "other" && !!versions?.some((v) => v.prerelease))));
  const preOpts = [
    { value: "stable", label: "Stable" },
    { value: "pre", label: "Pre-release" },
  ];
  function setPre(v: string) {
    if (!app) return;
    store.patchSettings({ preReleases: { ...(store.settings.preReleases ?? {}), [app.id]: v === "pre" } });
  }
  let reqSeq = 0;
  function loadVersions(aid: string) {
    const seq = ++reqSeq;
    versionsError = null;
    tryLoad(() => call("get_versions", { appId: aid })).then((r) => {
      if (seq !== reqSeq) return;
      if (r.ok) versions = r.value;
      else {
        versions = [];
        versionsError = r.error;
      }
    });
  }
  $effect(() => {
    const aid = app?.id;
    app?.version;
    if (!aid) return;
    loadVersions(aid);
  });
  /** The backend refreshed the cached history in the background: swap the list in without a spinner, and only when it differs. */
  function reloadQuiet() {
    const aid = app?.id;
    if (!aid) return;
    const seq = ++reqSeq;
    call("get_versions", { appId: aid })
      .then((r) => {
        if (seq === reqSeq && (versionsError || JSON.stringify(r) !== JSON.stringify(versions))) {
          versionsError = null;
          versions = r;
        }
      })
      .catch(() => {});
  }

  const installable = $derived((versions ?? []).filter((v) => !v.installed));
  let pick = $state("");
  $effect(() => {
    if (!app || !versions) return;
    if (!installable.some((v) => v.version === pick)) {
      const older = installable.find((v) => cmpVersion(v.version, app.version) < 0 && !v.prerelease);
      pick = (older ?? installable[0])?.version ?? "";
    }
  });
  const pickInfo = $derived(installable.find((v) => v.version === pick));
  const pickBack = $derived(!!app && !!pick && !sideBySide && cmpVersion(pick, app.version) < 0);
  const pickOpts = $derived(installable.map((v) => ({ value: v.version, label: `${v.version}${v.prerelease ? " (Pre-release)" : ""}${v.date ? ", " + dateShort(v.date) : ""}` })));

  let picker = $state<{ focus: () => void }>();
  onMount(() => {
    let offVersions = () => {};
    let gone = false;
    on("versions-changed", (e) => {
      if (app && e.id === app.id) reloadQuiet();
    }).then((u) => (gone ? u() : (offVersions = u)));
    if (ui.pickVersion) {
      ui.pickVersion = false;
      setTimeout(() => picker?.focus(), 250);
    }
    const k = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "Enter" && !modalOpen()) {
        e.preventDefault();
        doInstallPick();
      }
    };
    window.addEventListener("keydown", k);
    return () => {
      gone = true;
      offVersions();
      window.removeEventListener("keydown", k);
    };
  });

  function doInstallPick() {
    if (app && pick && !app.readOnly) installVersion(app, pick, pickInfo?.size ?? null);
  }
  function openNotes(url: string) {
    import("@tauri-apps/plugin-opener").then((m) => m.openUrl(url)).catch(() => window.open(url, "_blank"));
  }
  function rowAction(v: VersionInfo): { label: string; title: string } | null {
    if (!app || v.installed || app.readOnly || isDriver) return null;
    if (sideBySide) return { label: `Install ${v.version}`, title: `Install ${v.version} next to ${app.version}` };
    if (v.latest && cmpVersion(v.version, app.version) > 0) return null;
    if (cmpVersion(v.version, app.version) < 0) return app.canRollback ? { label: `Roll back to ${v.version}`, title: `Roll back to ${v.version}` } : null;
    return { label: `Install ${v.version}`, title: `Install ${v.version}` };
  }

  const verSplit = $derived(app ? splitVersion(app.version) : ["", ""]);
  const bigSize = $derived(!app ? 64 : verSplit[1] ? 64 : app.version.length > 7 ? 48 : 64);
  const lang = $derived(({ en_US: "English (US)", en_GB: "English (UK)", de_DE: "German", fr_FR: "French", es_ES: "Spanish", ja_JP: "Japanese" } as Record<string, string>)[store.settings.language] ?? store.settings.language);
  const statusLine = $derived.by(() => {
    if (!app) return "";
    if (listedOnly(app) && !job) return app.status === "missing" ? "Uninstaller missing" : "Installed";
    if (isDriver && !job && app.status === "update" && app.manualUrl) return `${app.latest} available from its maker`;
    if (!job && ignored) return app.latest && cmpVersion(app.latest, app.version) > 0 ? `Updates off, ${app.latest} is out` : "Updates off";
    switch (app.status) {
      case "update":
        return `${app.latest} available${app.updateSize ? ", " + bytes(app.updateSize) : ""}`;
      case "missing":
        return "Files missing";
      case "failed":
        return "Last update failed";
      case "current":
        return "Up to date";
      case "unknown":
        return "Latest version unknown";
      default:
        return job ? jobStatus(job) : "Working";
    }
  });
  const keepNote = $derived(app?.sizeOnDisk ? `Roll back without downloading again. Uses about ${bytes(app.sizeOnDisk)}.` : "Roll back without downloading again. Uses more disk space.");
  const pathParts = $derived((app?.installDir ?? "").split(/(?<=\\)/));
</script>

<section class="screen detail">
  {#if !app}
    <a class="back" href="#/installed"><Icon d={I.back} />Installed</a>
    <p class="gone">This app is not installed any more.</p>
  {:else}
    <Splash colors={[app.fg]} opacity={0.07} left lines />

    <a class="back" href="#/installed" title="Back  Alt Left" onclick={(e) => { e.preventDefault(); router.up("installed"); }}><Icon d={I.back} />Installed</a>

    <header class="hd" in:rise>
      <Mark app={app} size={80} fs={30} />
      <div class="ttl">
        <span class="nm">{app.name}</span>
        <span class="ver" style="font-size:{bigSize}px" title={app.version}>{verSplit[0]}{#if verSplit[1]}<span class="vb">{verSplit[1]}</span>{/if}</span>
        {#if aboutLine(app)}<span class="about">{aboutLine(app)}</span>{/if}
        {#if app.note}<span class="note">{app.note}</span>{/if}
      </div>
      <div class="side">
        <span class="stl" style:color={statusColor(app.status)}>
          <Dab color={statusColor(app.status)} pulse={app.status === "updating"} />{statusLine}{#if job}<span class="pc"><Num value={pct(job)} />%</span>{/if}
        </span>
        <div class="btns">
          {#if job}
            <Btn variant="ghost" disabled={!canPause(job)} onclick={() => send(job.state === "paused" ? "resume_job" : "pause_job", { jobId: job.id })} title={pauseTip(job)}>
              <Icon d={job.state === "paused" ? I.play : I.pause} />{job.state === "paused" ? "Resume" : "Pause"}
            </Btn>
            <Btn variant="secondary" disabled={!canCancel(job)} onclick={() => send("cancel_job", { jobId: job.id })} title={cancelTip(job)}>Cancel</Btn>
          {:else}
            {#if app.status !== "missing" && !isDriver}
              <Btn variant="ghost" title="Open {app.name}  O" onclick={() => openApp(app.id)}>Open</Btn>
            {/if}
            {#if ignored}
              <Btn variant="secondary" title="Show and install its updates again" onclick={() => setIgnored(app.id, false)}>Turn updates on</Btn>
            {/if}
            {#if isDriver && store.status.restartNeeded}
              <Btn variant="secondary" title="Restart the PC in a minute to finish installing drivers" onclick={() => send("restart_pc", {})}>Restart PC</Btn>
            {/if}
            {#if app.manualUrl}
              <Btn variant={app.status === "update" ? "primary" : "secondary"} title="Open {app.manualUrl}" onclick={() => send("open_driver_page", { id: app.id })}>{app.status === "update" ? (app.id === "driver:bios" ? "Download from maker" : `Get ${app.latest}`) : app.github ? "Release page" : "Maker's page"} <Icon d={I.external} size={14} width={2.5} /></Btn>
            {:else if app.status === "update"}
              <Btn variant="primary" hint="U" title="Update to {app.latest}  U" onclick={() => updateOne(app.id)}>Update to {app.latest}</Btn>
            {:else if app.status === "missing" && canRepair(app)}
              <Btn variant="primary" hint="R" title="Repair {app.name}  R" onclick={() => repairApp(app.id)}>Repair</Btn>
            {:else if app.status === "failed"}
              <Btn variant="primary" hint="R" title="Retry  R" onclick={() => { const f = store.failedJobs.find((x) => x.appId === app.id); if (f) send("retry_job", { jobId: f.id }); }}>Retry</Btn>
            {/if}
            {#if app.storeUrl}
              <Btn variant="secondary" title="Updates and removal happen in the Microsoft Store" onclick={() => openStorePage(app.id)}>Open in Microsoft Store <Icon d={I.external} size={14} width={2.5} /></Btn>
            {/if}
            {#if app.canUninstall}
              <Btn variant="danger" title="Uninstall {app.name}" onclick={() => askUninstall(app.id)}>{app.source === "programs" && app.status === "missing" ? "Remove from list" : "Uninstall"}</Btn>
            {/if}
          {/if}
        </div>
      </div>
    </header>

    {#if job}
      <div class="prog" in:rise><BrushBar value={job.progress} color={app.fg} height={6} /></div>
    {/if}

    <div class="cols">
      {#if listedOnly(app) || (isDriver && versions?.length === 1 && !versions[0].notes.length && !versions[0].notesUrl)}
      <section aria-label="About this program">
        {#if isDriver}
        <div class="sec-h"><span>About this driver</span></div>
        <p class="gone">{app.manualUrl ? "Its maker's page has the newest version, its changes and how to install it." : "Installed for a device in this PC. Windows Update and the makers are asked for a newer one with every check."}</p>
        {:else}
        <div class="sec-h"><span>About this program</span></div>
        <p class="gone">No catalog lists {app.name}, so its updates are not checked here. Uninstall runs its own uninstaller and then shows what it left behind, to clean up.</p>
        {/if}
      </section>
      {:else}
      <section aria-label="Version history">
        <div class="sec-h">
          <span>Version history</span>
          <span class="aside">{versions && !versionsError && !app.readOnly && !isDriver ?`${installable.length} ${installable.length === 1 ? "version" : "versions"} you can install` : ""}</span>
        </div>
        {#if !versions}
          {#each [0, 1, 2, 3] as i (i)}
            <div class="vrow skel" aria-hidden="true" style="animation-delay:{i * 100}ms"><span class="sk w1"></span><span class="sk w2"></span></div>
          {/each}
        {:else if versionsError}
          <p class="gone">Could not load the versions. Check the connection.</p>
          <div class="retry"><Btn variant="raised" size="md" onclick={() => app && loadVersions(app.id)}>Retry</Btn></div>
        {:else}
          {#each versions as v, i (v.version)}
            {@const act = rowAction(v)}
            <article class="vrow" class:hasnotes={v.notes.length > 0} animate:flip={{ duration: reduced() ? 0 : 200 }} use:enter={i}>
              <div class="vtop">
                <span class="vn">{preParts(v.version)[0]}{#if preParts(v.version)[1]}<span class="vs">{preParts(v.version)[1]}</span>{/if}</span>
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
                  <button class="lnk" title="Open the full release notes" onclick={() => openNotes(v.notesUrl!)}>Full release notes</button>
                {/if}
                {#if act}
                  <Btn variant="ghost" size="sm" title={act.title} onclick={() => installVersion(app, v.version, v.size)}>{act.label}</Btn>
                {/if}
              </div>
              {#if v.notes.length}
                <div class="notes"><NotesList notes={v.notes} /></div>
              {/if}
            </article>
          {/each}
        {/if}
      </section>
      {/if}

      <aside aria-label="Details">
        {#if preShown}
        <div class="blk">
          <span class="lbl" id="pre-lbl">Updates</span>
          <Segmented options={preOpts} value={preOn ? "pre" : "stable"} onchange={setPre} label="Stable or pre-release updates" />
        </div>
        {/if}
        {#if app.source === "other" && !app.isSelf}<OtherBuilds id={app.id} />{/if}
        {#if !app.readOnly && !listedOnly(app) && !isDriver}
        <div class="blk">
          <label for="ver-pick" class="lbl">Install a version</label>
          <div class="pk">
            <Select id="ver-pick" bind:this={picker} options={pickOpts} value={pick} onchange={(v) => (pick = v)} width="100%" label="Version to install" display={versions ? "No other versions" : "Loading"} />
            <Btn variant="raised" size="md" disabled={!pick || !!job} title="{pickBack ? 'Roll back to' : 'Install'} {pick}  Ctrl Enter" onclick={doInstallPick}>{pickBack ? "Roll back" : "Install"}</Btn>
          </div>
          <span class="hint">
            {#if sideBySide && !pickOpts.length}No other version is offered right now.{:else if sideBySide}Installs next to {app.version}, your {app.version} stays.{:else if pickBack}Your settings are saved first. After the rollback, Activity can put back the ones {pick} had.{:else if isAdobe}Your preferences and plugins stay as they are.{:else}Your settings stay as they are, and are saved first.{/if}
          </span>
        </div>
        {/if}

        <dl>
          {#if app.source === "programs" && app.status !== "missing" && app.tagline}<dt>Publisher</dt><dd>{app.tagline}</dd>{/if}
          {#if app.github}<dt>Updates from</dt><dd>GitHub releases of <span class="mono selectable">{app.github}</span></dd>{/if}
          {#if isDriver}
          <dt>Kind</dt>
          <dd>{app.tagline ?? "Driver"}</dd>
          <dt>Updated</dt>
          <dd>{app.updatedAt ? dateLong(app.updatedAt) : "Not by this app"}</dd>
          {:else}
          <dt>Location</dt>
          <dd>
            {#if app.installDir}
              <span class="path mono selectable" title={app.installDir}>{#each pathParts as part, i (i)}{part}<wbr />{/each}</span>
              <button class="lnk2" onclick={() => showInFolder(app.id)}>Show in folder</button>
            {:else}
              <span class="mut">Not known</span>
            {/if}
          </dd>
          {#if isAdobe}<dt>Language</dt><dd>{lang}</dd>{/if}
          {#if isBlender && app.channel}<dt>Channel</dt><dd class="cap">{app.channel === "lts" ? "Long-term support" : app.channel === "beta" ? "Pre-release" : "Stable"}</dd>{/if}
          <dt>Size on disk</dt>
          <dd>{app.sizeOnDisk ? bytes(app.sizeOnDisk) : "Not known"}</dd>
          <dt>Updated</dt>
          <dd>{app.updatedAt ? dateLong(app.updatedAt) : "Not known"}</dd>
          {/if}
        </dl>

        {#if app.readOnly || app.source === "programs" || isDriver}
          <!-- Steam or the Microsoft Store keeps this copy up to date; no catalog knows Other programs -->
        {:else if isBlender && app.channel === "beta"}
          <!-- a pre-release copy has no bug-fix releases to keep -->
        {:else if isBlender}
          <div class="keep">
            <div class="kt">
              <span id="keep-label" class="kn">Keep bug-fix versions</span>
              <span class="ks">A bug-fix update such as {app.version} to the next patch keeps the old copy.</span>
            </div>
            <Toggle checked={store.settings.blenderKeepPatchVersions} labelledby="keep-label" onchange={(v) => store.patchSettings({ blenderKeepPatchVersions: v })} />
          </div>
        {:else}
          <div class="keep">
            <div class="kt">
              <span id="keep-label" class="kn">Keep previous version</span>
              <span class="ks">{keepNote}</span>
            </div>
            <Toggle checked={store.settings.keepPreviousVersions} labelledby="keep-label" onchange={(v) => store.patchSettings({ keepPreviousVersions: v })} />
          </div>
        {/if}
      </aside>
    </div>
  {/if}
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
    margin-top: 32px;
    color: var(--text-3);
  }
  .retry {
    margin-top: 12px;
  }
  .hd {
    position: relative;
    display: flex;
    align-items: flex-end;
    gap: 24px;
    margin-top: 20px;
  }
  .ttl {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .nm {
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
    font-size: 28px;
    letter-spacing: -0.02em;
    font-weight: 500;
    color: var(--text-3);
  }
  .note {
    font-size: 13px;
    color: var(--text-3);
  }
  .about {
    font-size: 13px;
    line-height: 1.45;
    color: var(--text-2);
    max-width: 640px;
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
    white-space: nowrap;
  }
  .tx {
    position: relative;
  }
  .grow {
    flex: 1;
  }
  .lnk,
  .lnk2 {
    font-size: 12px;
    white-space: nowrap;
    color: var(--text-3);
    transition: color var(--t-fast);
  }
  .lnk:hover,
  .lnk2:hover {
    color: var(--text);
  }
  .notes {
    margin: 8px 0 0 86px;
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
  .w1 {
    width: 72px;
  }
  .w2 {
    width: 90px;
  }
  aside {
    display: flex;
    flex-direction: column;
    gap: 28px;
    min-width: 0;
  }
  .blk {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  /* a switch is as wide as its options, not the column */
  .blk > :global(.seg) {
    align-self: flex-start;
  }
  .lbl {
    font-size: 12px;
    color: var(--text-3);
  }
  .pk {
    display: flex;
    gap: 8px;
  }
  .pk :global(.select) {
    flex: 1;
    min-width: 0;
  }
  .hint {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-3);
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
  .cap {
    text-transform: none;
  }
  .path {
    font-size: 12px;
    line-height: 1.5;
    overflow-wrap: anywhere;
    color: var(--text);
  }
  .mut {
    color: var(--text-3);
  }
  .keep {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }
  .kt {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .kn {
    font-size: 13px;
    font-weight: 500;
  }
  .ks {
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
