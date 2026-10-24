<script lang="ts">
  // The Updater 3D's own page, like the page of any app: its version, its update, every release
  // with its notes (from its public GitHub project) and where it is on this PC.
  import { onMount } from "svelte";
  import Splash from "../components/Splash.svelte";
  import NotesList from "../components/NotesList.svelte";
  import Btn from "../components/Btn.svelte";
  import Dab from "../components/Dab.svelte";
  import Icon from "../components/Icon.svelte";
  import Paint from "../components/Paint.svelte";
  import { I } from "../lib/icons";
  import { call } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { router } from "../lib/router.svelte";
  import { updateSelf } from "../lib/actions";
  import { tryLoad } from "../lib/load";
  import { bytes, dateLong, preParts, splitVersion } from "../lib/format";
  import { enter, rise } from "../lib/motion";
  import type { SelfInfo, VersionInfo } from "../lib/types";

  let info = $state<SelfInfo | null>(null);
  let versions = $state<VersionInfo[] | null>(null);
  let versionsError = $state<string | null>(null);
  let checking = $state(false);

  const version = $derived(info?.version || store.status.appVersion || "");
  const ready = $derived(store.status.selfUpdate);
  const verSplit = $derived(splitVersion(version));
  const pathParts = $derived((info?.installDir ?? "").split(/(?<=\\)/));
  const statusLine = $derived(ui.selfUpdating ? "Downloading, restarts by itself" : ready ? `${ready} is ready` : "Up to date");
  const statusTint = $derived(ready ? "var(--warn)" : "var(--ok)");

  function loadVersions() {
    versionsError = null;
    tryLoad(() => call("self_versions", {})).then((r) => {
      if (r.ok) versions = r.value;
      else {
        versions = [];
        versionsError = r.error;
      }
    });
  }
  onMount(() => {
    call("self_info", {})
      .then((i) => (info = i))
      .catch(() => {});
    loadVersions();
  });
  // A new version found while the page is open shows in the history too.
  $effect(() => {
    if (ready && versions && !versions.some((v) => v.version === ready)) loadVersions();
  });

  async function checkNow() {
    checking = true;
    try {
      const v = await call("check_self_update", {});
      if (!v) store.toast("info", "The Updater 3D is up to date.");
    } catch (e) {
      store.toast("err", `Could not check for a new version: ${e}`);
    } finally {
      checking = false;
    }
  }
  function open(url: string) {
    import("@tauri-apps/plugin-opener").then((m) => m.openUrl(url)).catch(() => window.open(url, "_blank"));
  }
</script>

<section class="screen detail">
  <Splash colors={["#7C93D6"]} opacity={0.07} left lines />

  <a class="back" href="#/settings/about" title="Back  Alt Left" onclick={(e) => { e.preventDefault(); router.back(); }}><Icon d={I.back} />Back</a>

  <header class="hd" in:rise>
    <img class="logo" src="/app-icon.png" alt="" width="80" height="80" />
    <div class="ttl">
      <span class="nm">The Updater 3D</span>
      <span class="ver" title={version}>{verSplit[0]}{#if verSplit[1]}<span class="vb">{verSplit[1]}</span>{/if}</span>
      <span class="note">Keeps your apps, games and drivers up to date. It updates itself the same way.</span>
    </div>
    <div class="side">
      <span class="stl" style:color={statusTint}><Dab color={statusTint} pulse={ui.selfUpdating} />{statusLine}</span>
      <div class="btns">
        <Btn variant="raised" disabled={checking} title="Look for a new version of The Updater 3D" onclick={checkNow}>{checking ? "Checking" : "Check now"}</Btn>
        {#if ready}
          <Btn variant="primary" disabled={ui.selfUpdating} title="Update to {ready} and restart" onclick={updateSelf}>{ui.selfUpdating ? "Downloading" : `Update to ${ready}`}</Btn>
        {/if}
      </div>
    </div>
  </header>

  <div class="cols">
    <section aria-label="Version history">
      <div class="sec-h"><span>Version history</span></div>
      {#if !versions}
        {#each [0, 1, 2, 3] as i (i)}
          <div class="vrow skel" aria-hidden="true" style="animation-delay:{i * 100}ms"><span class="sk w1"></span><span class="sk w2"></span></div>
        {/each}
      {:else if versionsError || !versions.length}
        <p class="gone">{versionsError ? "Could not load the versions. Check the connection." : "No releases could be read right now."}</p>
        <div class="retry"><Btn variant="raised" size="md" onclick={loadVersions}>Retry</Btn></div>
      {:else}
        {#each versions as v, i (v.version)}
          <article class="vrow" class:hasnotes={v.notes.length > 0} use:enter={i}>
            <div class="vtop">
              <span class="vn">{preParts(v.version)[0]}{#if preParts(v.version)[1]}<span class="vs">{preParts(v.version)[1]}</span>{/if}</span>
              <span class="vd">{dateLong(v.date)}</span>
              {#if v.installed}
                <span class="tag" style="color:var(--text)"><Paint kind="swatch" fill="#232323" /><span class="tx">Installed</span></span>
              {/if}
              {#if v.latest}
                <span class="tag" style="color:var(--warn)"><Paint kind="swatch" fill="#232323" /><span class="tx">Latest</span></span>
              {/if}
              <span class="grow"></span>
              {#if v.notesUrl}
                <button class="lnk" title="Open the release on GitHub" onclick={() => open(v.notesUrl!)}>Full release notes</button>
              {/if}
            </div>
            {#if v.notes.length}
              <div class="notes"><NotesList notes={v.notes} /></div>
            {/if}
          </article>
        {/each}
      {/if}
    </section>

    <aside aria-label="Details">
      <dl>
        <dt>Publisher</dt>
        <dd>CIoudGuy</dd>
        <dt>Location</dt>
        <dd>
          {#if info?.installDir}
            <span class="path mono selectable" title={info.installDir}>{#each pathParts as part, i (i)}{part}<wbr />{/each}</span>
          {:else}
            <span class="mut">Not known</span>
          {/if}
        </dd>
        <dt>Size on disk</dt>
        <dd>{info?.sizeOnDisk ? bytes(info.sizeOnDisk) : "Not known"}</dd>
        <dt>Updated</dt>
        <dd>{info?.updatedAt ? dateLong(info.updatedAt) : "Not known"}</dd>
        <dt>Updates from</dt>
        <dd>Its own update feed, checked with every check</dd>
        {#if info?.projectUrl}
          <dt>Project</dt>
          <dd><button class="lnk2" title="Open {info.projectUrl}" onclick={() => open(info!.projectUrl)}>GitHub <Icon d={I.external} size={12} width={2.5} /></button></dd>
        {/if}
      </dl>
    </aside>
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
  .logo {
    flex: none;
    width: 80px;
    height: 80px;
    border-radius: 22%;
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
    font-size: 64px;
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
  .btns {
    display: flex;
    gap: 8px;
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
    display: inline-flex;
    align-items: center;
    gap: 4px;
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
  .path {
    font-size: 12px;
    line-height: 1.5;
    overflow-wrap: anywhere;
    color: var(--text);
  }
  .mut {
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
