<script lang="ts">
  import { flip } from "svelte/animate";
  import { onMount } from "svelte";
  import Paint from "../components/Paint.svelte";
  import Icon from "../components/Icon.svelte";
  import Segmented from "../components/Segmented.svelte";
  import CatalogCard from "../components/CatalogCard.svelte";
  import { BRUSH } from "../lib/paths";
  import { I } from "../lib/icons";
  import { call } from "../lib/api";
  import { OTHER_NOTE, store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { bytes, countOf, driveOf } from "../lib/format";
  import { enter, reduced } from "../lib/motion";
  import type { CatalogEntry } from "../lib/types";

  const catalog = $derived(ui.catalog);
  let searchInput = $state<HTMLInputElement>();
  onMount(() => {
    // a list kept from an earlier visit is shown at once and refreshed in the background
    call("get_catalog", {}).then((c) => (ui.catalog = c));
    ui.searchEl = searchInput ?? null;
    return () => {
      if (ui.searchEl === searchInput) ui.searchEl = null;
    };
  });

  const q = $derived(ui.getQuery.trim().toLowerCase());
  const s = $derived(store.settings);

  type Group = "Adobe" | "Craft apps" | "Blender" | "More apps" | "Other apps";
  const SECTIONS: { group: Group; title: string; from: string; color: string; enabled: () => boolean }[] = [
    { group: "Adobe", title: "Adobe apps", from: "from Creative Cloud", color: "#D9706A", enabled: () => s.sources.adobe },
    { group: "Craft apps", title: "Craft apps", from: "from ArtCraft", color: "#D9B26A", enabled: () => s.sources.craft },
    { group: "Blender", title: "Blender", from: "from blender.org", color: "#E08A3C", enabled: () => s.sources.blender },
    { group: "More apps", title: "More apps", from: "OBS Studio, Krita, FreeCAD, Godot, Unity", color: "#6FB1E3", enabled: () => s.sources.tools },
    { group: "Other apps", title: "Other apps", from: "from Microsoft's winget catalog", color: "#8FA3B8", enabled: () => s.sources.other },
  ];

  // Other apps: a search also looks through the winget catalog. A plain flag, so settings
  // updates that change nothing here do not restart the search.
  const otherOn = $derived(s.sources.other);
  let searchSeq = 0;
  $effect(() => {
    const query = q;
    const on = otherOn;
    const n = ++searchSeq;
    if (!on || query.length < 2) {
      ui.otherFound = [];
      return;
    }
    const t = setTimeout(() => {
      call("search_other_apps", { query })
        .then((r) => {
          if (n === searchSeq) ui.otherFound = r;
        })
        .catch(() => {});
    }, 250);
    return () => clearTimeout(t);
  });

  /** Search and the Beta settings decide what is listed at all. */
  function listed(c: CatalogEntry): boolean {
    if (q && !c.name.toLowerCase().includes(q) && !(c.tagline ?? "").toLowerCase().includes(q)) return false;
    if (c.prerelease && c.source === "blender" && s.blenderChannel !== "beta") return false;
    if (c.prerelease && c.source === "craft" && !s.craftPrereleases) return false;
    return true;
  }
  const onSources = $derived(SECTIONS.filter((x) => x.enabled()));
  const found = $derived.by(() => {
    const base = (catalog ?? []).filter(listed);
    if (!q) return base;
    const have = new Set(base.map((c) => c.id));
    return [...base, ...ui.otherFound.filter((c) => !have.has(c.id))];
  });
  const counts = $derived(Object.fromEntries(SECTIONS.map((x) => [x.group, found.filter((c) => c.group === x.group).length])) as Record<Group, number>);
  // a filter on a source that was switched off falls back to All
  const filter = $derived(ui.getGroup !== "all" && onSources.some((x) => x.group === ui.getGroup) ? ui.getGroup : "all");
  const filterOpts = $derived([
    { value: "all", label: "All", badge: onSources.reduce((n, x) => n + counts[x.group], 0) },
    ...onSources.map((x) => ({ value: x.group as string, label: x.group, badge: counts[x.group] })),
  ]);

  const sections = $derived(
    onSources
      .filter((x) => filter === "all" || filter === x.group)
      .map((x) => {
        const all = found.filter((c) => c.group === x.group);
        // what you can still install comes first; Blender keeps its own order (stable, beta, LTS, older)
        const items = x.group === "Blender" ? all : [...all.filter((c) => !c.installed), ...all.filter((c) => c.installed)];
        return { ...x, items };
      })
      // Other apps stay listed without a search: the note says how to find more.
      .filter((x) => x.items.length > 0 || (x.group === "Other apps" && !q)),
  );
  const toInstall = $derived(sections.reduce((n, x) => n + x.items.filter((c) => !c.installed).length, 0));
  const have = $derived(sections.reduce((n, x) => n + x.items.filter((c) => c.installed).length, 0));
  const disks = $derived(
    [...store.status.disks]
      .sort((a, b) => b.free - a.free)
      .map((d, i) => (i === 0 ? `${bytes(d.free)} free on ${driveOf(d.path)}` : `${bytes(d.free)} on ${driveOf(d.path)}`))
      .join(", "),
  );
</script>

<section class="screen tight get">
  <div class="head">
    <div class="tt">
      <h1 class="h1">Get apps</h1>
      <span class="sub">{catalog ? `${countOf(toInstall, "app")} to install${have ? `, ${have} already installed` : ""}.` : "Loading the catalog."} {disks}</span>
    </div>
    <label class="search">
      <Icon d={I.search} />
      <input bind:this={searchInput} bind:value={ui.getQuery} aria-label="Search apps to install" placeholder="Search apps" spellcheck="false" autocomplete="off" />
      {#if !ui.compact}<span class="kbd">Ctrl F</span>{/if}
    </label>
  </div>

  {#if catalog}
    <div class="filters">
      <Segmented options={filterOpts} value={filter} onchange={(v) => (ui.getGroup = v as typeof ui.getGroup)} label="Show apps from" />
    </div>
  {/if}

  {#if !catalog}
    <div class="grid skelgrid" aria-hidden="true">
      {#each Array(8) as _, i (i)}<div class="card skel" style="animation-delay:{i * 80}ms"><Paint kind="blob" fill="#1c1c1c" /></div>{/each}
    </div>
  {:else if sections.length === 0}
    <p class="none">{q ? `No apps match "${ui.getQuery}".` : "Nothing to show for this source."}</p>
  {/if}

  {#each sections as sec, si (sec.group)}
    <h2 class="grp" aria-label="{sec.title}, {sec.from}">
      <span class="gt">
        <svg class="brush" viewBox="0 0 400 24" preserveAspectRatio="none" aria-hidden="true"><path d={BRUSH} fill={sec.color} fill-opacity="0.5" /></svg>
        <span class="gn">{sec.title}</span>
      </span>
      <span class="from">, {sec.from}</span>
      {#if sec.group === "Adobe" && store.ccDown}<span class="warn">Creative Cloud services are not running</span>{:else}<span class="cnt">{countOf(sec.items.length, "app")}</span>{/if}
    </h2>
    {#if sec.group === "Other apps"}<p class="srcnote">{OTHER_NOTE}{q ? "" : " Search to find more of its apps."}</p>{/if}
    <div class="grid">
      {#each sec.items as c, i (c.id)}
        <div class="slot" animate:flip={{ duration: reduced() ? 0 : 220 }} use:enter={i}>
          <CatalogCard {c} eager={si === 0 && i < 8} />
        </div>
      {/each}
    </div>
  {/each}
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
    min-width: 0;
  }
  .search {
    width: 240px;
    height: 32px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-3);
    flex: none;
    transition: background-color var(--t-fast) ease;
  }
  .search:hover,
  .search:focus-within {
    background: var(--surface-2);
  }
  .search input {
    flex: 1;
    min-width: 0;
    height: 30px;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--text);
    font-size: 13px;
  }
  .search input::placeholder {
    color: var(--text-3);
  }
  .kbd {
    font-size: 12px;
    color: var(--text-4);
  }
  .filters {
    margin-top: 24px;
    display: flex;
  }
  .grp {
    margin-top: 32px;
    height: 24px;
    display: flex;
    align-items: baseline;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-2);
    white-space: pre;
  }
  /* section title with a short painted stroke under it */
  .gt {
    position: relative;
    display: inline-block;
    padding-bottom: 2px;
  }
  .gn {
    position: relative;
    color: var(--text);
  }
  .brush {
    position: absolute;
    left: -3px;
    right: -3px;
    bottom: -3px;
    width: calc(100% + 6px);
    height: 5px;
    overflow: visible;
  }
  .grp .from {
    font-weight: 400;
    color: var(--text-3);
  }
  .srcnote {
    margin-top: 4px;
    font-size: 12px;
    color: var(--text-3);
  }
  .grp .cnt,
  .grp .warn {
    margin-left: auto;
    font-weight: 400;
    color: var(--text-3);
  }
  .grp .warn {
    color: var(--err);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 16px;
    margin-top: 14px;
  }
  .slot {
    min-width: 0;
  }
  .skelgrid {
    margin-top: 32px;
  }
  .card {
    position: relative;
    height: 168px;
    border-radius: var(--r-card);
    overflow: hidden;
  }
  .card.skel {
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .none {
    margin-top: 48px;
    color: var(--text-3);
  }
  @media (max-width: 999px) {
    .grid {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
    .search {
      width: 200px;
    }
  }
</style>
