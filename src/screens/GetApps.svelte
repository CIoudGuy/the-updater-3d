<script lang="ts">
  import { flip } from "svelte/animate";
  import { onMount } from "svelte";
  import Paint from "../components/Paint.svelte";
  import Icon from "../components/Icon.svelte";
  import Segmented from "../components/Segmented.svelte";
  import CatalogCard from "../components/CatalogCard.svelte";
  import { BRUSH } from "../lib/paths";
  import { I } from "../lib/icons";
  import { call, on } from "../lib/api";
  import { OTHER_NOTE, store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { bytes, countOf, driveOf } from "../lib/format";
  import { enter, reduced } from "../lib/motion";
  import type { CatalogEntry } from "../lib/types";
  import { tryLoad } from "../lib/load";
  import { allSourcesOff } from "../lib/view";
  import { BROWSE_PAGE, LETTERS, appendPage, catalogNote, patchEntries } from "../lib/browse";

  const catalog = $derived(ui.catalog);
  /** the catalog could not be loaded and none is kept from an earlier visit */
  let catalogError = $state<string | null>(null);
  function loadCatalog() {
    tryLoad(() => call("get_catalog", {})).then((r) => {
      if (r.ok) {
        catalogError = null;
        ui.catalog = r.value;
      } else if (!ui.catalog) catalogError = r.error;
    });
  }
  let searchInput = $state<HTMLInputElement>();
  onMount(() => {
    // a list kept from an earlier visit is shown at once and refreshed in the background
    loadCatalog();
    ui.searchEl = searchInput ?? null;
    // icons found in the background and the rest of the starter list arrive after the first paint
    let off: (() => void) | undefined;
    let gone = false;
    on("catalog_changed", () => {
      loadCatalog();
      if (otherOn && q.length >= 2) runSearch(q, searchSeq);
      if (otherOn && ui.otherMode === "browse") refreshBrowseSoon();
    }).then((u) => (gone ? u() : (off = u)));
    // descriptions and install verdicts of winget packages, read in the background
    let offDetails: (() => void) | undefined;
    on("other_details", (p) => {
      const f = patchEntries(ui.otherFound, p);
      if (f) ui.otherFound = f;
      const b = ui.otherBrowse && patchEntries(ui.otherBrowse.entries, p);
      if (b && ui.otherBrowse) ui.otherBrowse = { ...ui.otherBrowse, entries: b };
      const c = ui.catalog && patchEntries(ui.catalog, p);
      if (c) ui.catalog = c;
    }).then((u) => (gone ? u() : (offDetails = u)));
    if (ui.otherPackages === null) loadPackages();
    return () => {
      gone = true;
      off?.();
      offDetails?.();
      clearTimeout(refreshTimer);
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
    const t = setTimeout(() => runSearch(query, n), 250);
    return () => clearTimeout(t);
  });
  function runSearch(query: string, n: number) {
    call("search_other_apps", { query })
      .then((r) => {
        if (n === searchSeq) ui.otherFound = r;
      })
      .catch(() => {});
  }

  // Other apps, Browse all: the whole winget catalog by category and first letter, 60 at a time
  // as the list is scrolled. The pages loaded stay across visits.
  const browsing = $derived(otherOn && !q && ui.otherMode === "browse");
  let browseSeq = 0;
  let browseBusy = $state(false);
  let browseError = $state<string | null>(null);
  function loadPackages() {
    call("browse_other_apps", { category: null, letter: null, offset: 0, limit: 1 })
      .then((r) => {
        if (r.packages) ui.otherPackages = r.packages;
      })
      .catch(() => {});
  }
  function loadBrowse(category: string | null, letter: string | null, more: boolean) {
    const n = ++browseSeq;
    const have = more && ui.otherBrowse ? ui.otherBrowse.entries : [];
    browseBusy = true;
    browseError = null;
    call("browse_other_apps", { category, letter, offset: have.length, limit: BROWSE_PAGE })
      .then((r) => {
        if (n !== browseSeq) return;
        if (r.packages) ui.otherPackages = r.packages;
        ui.otherBrowse = { category, letter, entries: appendPage(have, r.entries), total: r.total, categories: r.categories };
      })
      .catch((e) => {
        if (n === browseSeq) browseError = String(e?.message ?? e);
      })
      .finally(() => {
        if (n === browseSeq) browseBusy = false;
      });
  }
  /** found icons: the pages already shown are read again, in one go */
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  function refreshBrowseSoon() {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
      const b = ui.otherBrowse;
      if (!b || browseBusy || !b.entries.length) return;
      const n = ++browseSeq;
      call("browse_other_apps", { category: b.category, letter: b.letter, offset: 0, limit: b.entries.length })
        .then((r) => {
          if (n === browseSeq && ui.otherBrowse) ui.otherBrowse = { ...ui.otherBrowse, entries: r.entries, total: r.total, categories: r.categories };
        })
        .catch(() => {});
    }, 600);
  }
  function setBrowse(category: string | null, letter: string | null) {
    ui.otherBrowse = ui.otherBrowse ? { ...ui.otherBrowse, category, letter, entries: [], total: 0 } : null;
    loadBrowse(category, letter, false);
  }
  $effect(() => {
    if (browsing && !ui.otherBrowse) loadBrowse(null, null, false);
  });
  const browseMore = $derived(!!ui.otherBrowse && ui.otherBrowse.entries.length < ui.otherBrowse.total);
  /** loads the next page when the end of the list scrolls into view */
  function sentinel(el: HTMLElement) {
    const io = new IntersectionObserver(
      (es) => {
        const b = ui.otherBrowse;
        if (es.some((e) => e.isIntersecting) && b && !browseBusy && !browseError && b.entries.length < b.total) loadBrowse(b.category, b.letter, true);
      },
      { rootMargin: "600px 0px" },
    );
    io.observe(el);
    return { destroy: () => io.disconnect() };
  }
  const modeOpts = [
    { value: "popular" as const, label: "Popular" },
    { value: "browse" as const, label: "Browse all" },
  ];

  /** Search and the Beta settings decide what is listed at all. */
  function listed(c: CatalogEntry): boolean {
    if (q && !c.name.toLowerCase().includes(q) && !(c.tagline ?? "").toLowerCase().includes(q)) return false;
    if (c.prerelease && c.source === "blender" && s.blenderChannel !== "beta") return false;
    if (c.prerelease && c.source === "craft" && !s.craftPrereleases) return false;
    return true;
  }
  const onSources = $derived(SECTIONS.filter((x) => x.enabled()));
  const sourcesOff = $derived(allSourcesOff(s.sources));
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

  // Other apps is a long list (a few hundred): the first ones show at once, the rest on a click.
  const PAGE = 60;
  let otherShown = $state(PAGE);
  $effect(() => {
    q;
    otherShown = PAGE;
  });

  const sections = $derived(
    onSources
      .filter((x) => filter === "all" || filter === x.group)
      .map((x) => {
        const all = found.filter((c) => c.group === x.group);
        // what you can still install comes first; Blender keeps its own order (stable, beta, LTS, older)
        const items = x.group === "Blender" ? all : [...all.filter((c) => !c.installed), ...all.filter((c) => c.installed)];
        if (x.group === "Other apps" && browsing) {
          // the page summary keeps counting the starter list; the whole catalog is counted below
          return { ...x, items: ui.otherBrowse?.entries ?? [], total: ui.otherBrowse?.total ?? 0, all: items };
        }
        const cut = x.group === "Other apps" && items.length > otherShown;
        return { ...x, items: cut ? items.slice(0, otherShown) : items, total: items.length, all: items };
      })
      // Other apps stay listed without a search: the note says how to find more.
      .filter((x) => x.items.length > 0 || (x.group === "Other apps" && !q)),
  );
  const toInstall = $derived(sections.reduce((n, x) => n + x.all.filter((c) => !c.installed).length, 0));
  const have = $derived(sections.reduce((n, x) => n + x.all.filter((c) => c.installed).length, 0));
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

  {#if !catalog && catalogError}
    <p class="none">Could not load the catalog. {catalogError} <button class="lnk" onclick={() => { catalogError = null; loadCatalog(); }}>Retry</button></p>
  {:else if !catalog}
    <div class="grid skelgrid" aria-hidden="true">
      {#each Array(8) as _, i (i)}<div class="card skel" style="animation-delay:{i * 80}ms"><Paint kind="blob" fill="#1c1c1c" /></div>{/each}
    </div>
  {:else if sections.length === 0}
    <p class="none">
      {#if sourcesOff}
        Every source is switched off. <a class="lnk" href="#/settings/sources">Turn some on in Settings, Sources</a>.
      {:else}
        {q ? `No apps match "${ui.getQuery}".` : "Nothing to show for this source."}
      {/if}
    </p>
  {/if}

  {#each sections as sec, si (sec.group)}
    <h2 class="grp" aria-label="{sec.title}, {sec.from}">
      <span class="gt">
        <svg class="brush" viewBox="0 0 400 24" preserveAspectRatio="none" aria-hidden="true"><path d={BRUSH} fill={sec.color} fill-opacity="0.5" /></svg>
        <span class="gn">{sec.title}</span>
      </span>
      <span class="from">, {sec.from}</span>
      {#if sec.group === "Adobe" && store.ccDown}<span class="warn">Creative Cloud services are not running</span>{:else}<span class="cnt">{countOf(sec.total, "app")}</span>{/if}
    </h2>
    {#if sec.group === "Other apps"}
      <p class="srcnote">{OTHER_NOTE}{q ? "" : browsing ? "" : " Browse all or search to find the rest."}</p>
      {#if !q}
        <div class="omode">
          <Segmented options={modeOpts} value={ui.otherMode} onchange={(v) => (ui.otherMode = v)} label="Other apps to show" />
          {#if ui.otherPackages}<span class="size">{catalogNote(ui.otherPackages)}</span>{/if}
        </div>
      {/if}
      {#if browsing}
        {@const b = ui.otherBrowse}
        <div class="chips" role="group" aria-label="Category">
          <button class="chip" class:on={!b?.category} aria-pressed={!b?.category} onclick={() => setBrowse(null, b?.letter ?? null)}>All</button>
          {#each b?.categories ?? [] as k (k.name)}
            {#if k.count > 0}
              <button class="chip" class:on={b?.category === k.name} aria-pressed={b?.category === k.name} onclick={() => setBrowse(k.name, b?.letter ?? null)}>{k.name}<span class="n">{k.count.toLocaleString("en-US")}</span></button>
            {/if}
          {/each}
        </div>
        <div class="letters" role="group" aria-label="First letter">
          <button class="let" class:on={!b?.letter} aria-pressed={!b?.letter} onclick={() => setBrowse(b?.category ?? null, null)}>All</button>
          {#each LETTERS as l (l)}
            <button class="let" class:on={b?.letter === l} aria-pressed={b?.letter === l} title={l === "#" ? "Names that do not start with A to Z" : `Names that start with ${l}`} onclick={() => setBrowse(b?.category ?? null, l)}>{l}</button>
          {/each}
        </div>
      {/if}
    {/if}
    {#if sec.group === "Other apps" && browsing}
      <div class="grid">
        {#each sec.items as c, i (c.id)}
          <div class="slot" use:enter={i % BROWSE_PAGE}>
            <CatalogCard {c} eager={i < 8} />
          </div>
        {/each}
        {#if browseBusy}
          {#each Array(sec.items.length ? 4 : 8) as _, i (i)}<div class="card skel" aria-hidden="true" style="animation-delay:{i * 80}ms"><Paint kind="blob" fill="#1c1c1c" /></div>{/each}
        {/if}
      </div>
      {#if browseError}
        <p class="none">Could not load the catalog. {browseError} <button class="lnk" onclick={() => ui.otherBrowse ? loadBrowse(ui.otherBrowse.category, ui.otherBrowse.letter, true) : loadBrowse(null, null, false)}>Retry</button></p>
      {:else if !browseBusy && ui.otherBrowse && ui.otherBrowse.total === 0}
        <p class="none">{ui.otherBrowse.letter ? `No apps here start with ${ui.otherBrowse.letter}.` : "No apps in this category."}</p>
      {/if}
      {#if browseMore}<div class="sentinel" use:sentinel aria-hidden="true"></div>{/if}
    {:else}
      <div class="grid">
        {#each sec.items as c, i (c.id)}
          <div class="slot" animate:flip={{ duration: reduced() ? 0 : 220 }} use:enter={i}>
            <CatalogCard {c} eager={si === 0 && i < 8} />
          </div>
        {/each}
      </div>
      {#if sec.items.length < sec.total}
        <button class="more" onclick={() => (otherShown += PAGE)}>Show {Math.min(PAGE, sec.total - sec.items.length)} more of {sec.total}</button>
      {/if}
    {/if}
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
  .omode {
    margin-top: 12px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .omode .size {
    font-size: 12px;
    color: var(--text-3);
  }
  .chips,
  .letters {
    margin-top: 12px;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip,
  .let {
    height: 28px;
    padding: 0 10px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text-2);
    font-size: 12px;
    font-weight: 500;
    transition: background-color var(--t-fast) ease, color var(--t-fast) ease;
  }
  .let {
    min-width: 26px;
    padding: 0 6px;
  }
  .chip:hover,
  .let:hover {
    background: var(--surface);
    color: var(--text);
  }
  .chip.on,
  .let.on {
    background: var(--surface-2);
    color: var(--text);
  }
  .chip .n {
    margin-left: 6px;
    color: var(--text-3);
    font-weight: 400;
  }
  .sentinel {
    height: 1px;
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
  .more {
    align-self: flex-start;
    margin-top: 16px;
    height: 32px;
    padding: 0 14px;
    border: 0;
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-2);
    font-size: 13px;
    transition: background-color var(--t-fast) ease;
  }
  .more:hover {
    background: var(--surface-2);
    color: var(--text);
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
  .none .lnk {
    color: var(--text);
    text-decoration: underline;
    text-decoration-color: var(--text-4);
    text-underline-offset: 3px;
  }
  .none .lnk:hover {
    text-decoration-color: var(--text);
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
