<script lang="ts">
  import { flip } from "svelte/animate";
  import { onMount, untrack } from "svelte";
  import Paint from "../components/Paint.svelte";
  import Icon from "../components/Icon.svelte";
  import Segmented from "../components/Segmented.svelte";
  import Select from "../components/Select.svelte";
  import CatalogCard from "../components/CatalogCard.svelte";
  import VirtualGrid from "../components/VirtualGrid.svelte";
  import { BRUSH } from "../lib/paths";
  import { I } from "../lib/icons";
  import { call, on } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import { ui } from "../lib/ui.svelte";
  import { bytes, countOf, driveOf } from "../lib/format";
  import { enter, reduced } from "../lib/motion";
  import type { CatalogEntry } from "../lib/types";
  import { tryLoad } from "../lib/load";
  import { allSourcesOff } from "../lib/view";
  import { BROWSE_PAGE, LETTERS, datedNote, mergeEntries, pagesToLoad, patchEntries, patchIcons, placePage, rankEntries, sameKey, sortOptions, type BrowseKey, type OtherSort } from "../lib/browse";

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
    // the rest of the starter list and changed installs arrive after the first paint
    let off: (() => void) | undefined;
    let gone = false;
    on("catalog_changed", () => {
      loadCatalog();
      if (otherOn) refreshBrowseSoon();
    }).then((u) => (gone ? u() : (off = u)));
    // icons found in the background, a few at a time: only those cards change, nothing moves
    let offIcons: (() => void) | undefined;
    on("other_icons", (p) => {
      const b = ui.otherBrowse && patchIcons(ui.otherBrowse.entries, p);
      if (b && ui.otherBrowse) ui.otherBrowse = { ...ui.otherBrowse, entries: b };
      const c = ui.catalog && patchIcons(ui.catalog, p);
      if (c) ui.catalog = c;
    }).then((u) => (gone ? u() : (offIcons = u)));
    // descriptions and install verdicts of winget packages, read in the background
    let offDetails: (() => void) | undefined;
    on("other_details", (p) => {
      const b = ui.otherBrowse && patchEntries(ui.otherBrowse.entries, p);
      if (b && ui.otherBrowse) ui.otherBrowse = { ...ui.otherBrowse, entries: b };
      const c = ui.catalog && patchEntries(ui.catalog, p);
      if (c) ui.catalog = c;
    }).then((u) => (gone ? u() : (offDetails = u)));
    return () => {
      gone = true;
      off?.();
      offDetails?.();
      offIcons?.();
      clearTimeout(refreshTimer);
      clearTimeout(fillTimer);
      clearTimeout(queryTimer);
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

  // Other apps: the whole winget catalog, popular apps first, a page at a time as the list is
  // scrolled. A search goes through all of it on the backend, a moment after the last key, so
  // typing stays quick; the curated sources above are filtered here at once.
  const otherOn = $derived(s.sources.other);
  let otherQuery = $state(untrack(() => ui.otherBrowse?.query ?? ""));
  let queryTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const query = q;
    clearTimeout(queryTimer);
    if (query === untrack(() => otherQuery)) return;
    // clearing the search shows the whole list again at once
    if (!query) otherQuery = "";
    else queryTimer = setTimeout(() => (otherQuery = query), 150);
  });
  const otherKey = $derived<BrowseKey>({ query: otherQuery, category: ui.otherCategory, letter: ui.otherLetter, sort: ui.otherSort });

  // The list has a place for every entry from its first page on, so the scrollbar spans all of
  // it; pages are loaded for the places in view (and the screens around them) as the list is
  // scrolled or dragged anywhere, and loaded pages are kept.
  let browseSeq = 0;
  let browseBusy = $state(false);
  let browseError = $state<string | null>(null);
  /** pages asked for and not back yet, by start offset, for list `browseSeq` */
  const pending = new Set<number>();
  let letterRow = $state<HTMLElement>();
  /** places in view and kept around them, from the grid */
  let range = { from: 0, to: 0, keepFrom: 0, keepTo: 0 };
  const req = (key: BrowseKey, offset: number, limit: number) => call("browse_other_apps", { query: key.query || null, category: key.category, letter: key.letter, sort: key.sort, offset, limit });
  function loadBrowse(key: BrowseKey) {
    const n = ++browseSeq;
    pending.clear();
    browseBusy = true;
    browseError = null;
    req(key, 0, BROWSE_PAGE)
      .then((r) => {
        if (n !== browseSeq) return;
        if (r.packages) ui.otherPackages = r.packages;
        const fresh = !ui.otherBrowse || !sameKey(ui.otherBrowse, key);
        ui.otherBrowse = { ...key, entries: placePage([], r.total, 0, r.entries), total: r.total, categories: r.categories, dated: r.dated };
        // a new search or filter shows its list from the start, not from where the old one was
        if (fresh && letterRow && letterRow.getBoundingClientRect().top < 0) letterRow.scrollIntoView({ block: "start" });
        fillSoon();
      })
      .catch((e) => {
        if (n === browseSeq) browseError = String(e?.message ?? e);
      })
      .finally(() => {
        if (n === browseSeq) browseBusy = false;
      });
  }
  /** Load the pages the places in view need, then the ones kept around them; a few at a time. */
  function fill() {
    const b = ui.otherBrowse;
    if (!b || !browseCurrent || browseBusy) return;
    const n = browseSeq;
    const key: BrowseKey = { query: b.query, category: b.category, letter: b.letter, sort: b.sort };
    const want = pagesToLoad(b.entries, range.from, range.to, BROWSE_PAGE, pending);
    for (const p of pagesToLoad(b.entries, range.keepFrom, range.keepTo, BROWSE_PAGE, pending)) if (!want.includes(p)) want.push(p);
    for (const offset of want.slice(0, Math.max(0, 3 - pending.size))) {
      pending.add(offset);
      req(key, offset, BROWSE_PAGE)
        .then((r) => {
          const cur = ui.otherBrowse;
          if (n !== browseSeq || !cur || !sameKey(cur, key)) return;
          ui.otherBrowse = { ...cur, entries: placePage(cur.entries, r.total, offset, r.entries), total: r.total };
          browseError = null;
        })
        .catch((e) => {
          if (n === browseSeq) browseError = String(e?.message ?? e);
        })
        .finally(() => {
          if (n !== browseSeq) return;
          pending.delete(offset);
          if (!browseError) fillSoon();
        });
    }
  }
  /** While the list is flung, only the pages where it comes to rest are asked for. */
  let fillTimer: ReturnType<typeof setTimeout> | undefined;
  function fillSoon(ms = 60) {
    clearTimeout(fillTimer);
    fillTimer = setTimeout(fill, ms);
  }
  function onRange(r: typeof range) {
    range = r;
    fillSoon();
  }
  // a new search, category, letter or order starts the list again; the old one stays on screen
  // until the new first page is there
  $effect(() => {
    if (!otherOn) return;
    const key = otherKey;
    const b = untrack(() => ui.otherBrowse);
    if (!b || !sameKey(b, key)) untrack(() => loadBrowse(key));
  });
  /** the list shown is the one asked for (not an older one while the new one loads) */
  const browseCurrent = $derived(!!ui.otherBrowse && sameKey(ui.otherBrowse, otherKey));

  /** the catalog changed on the backend (installs, the starter list): the pages in view are read
   * again and merged in by id, so the list keeps its order, its place and the lines above it */
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  function refreshBrowseSoon() {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
      const b = ui.otherBrowse;
      if (!b || browseBusy || !b.entries.length || !browseCurrent) return;
      const n = browseSeq;
      const key: BrowseKey = { query: b.query, category: b.category, letter: b.letter, sort: b.sort };
      const from = range.from - (range.from % BROWSE_PAGE);
      const limit = Math.min(200, Math.max(BROWSE_PAGE, range.to - from));
      req(key, from, limit)
        .then((r) => {
          if (n === browseSeq && ui.otherBrowse && sameKey(ui.otherBrowse, key)) ui.otherBrowse = { ...ui.otherBrowse, entries: mergeEntries(ui.otherBrowse.entries, r.entries) };
        })
        .catch(() => {});
    }, 600);
  }

  /** the first cards of a new list fade in once; cards met while scrolling are simply there */
  const animated = new Set<string>();
  function enterOnce(node: HTMLElement, a: { id: string; i: number }) {
    if (a.i >= 12 || animated.has(a.id)) return;
    animated.add(a.id);
    enter(node, a.i);
  }

  const sortOpts = $derived(sortOptions(!!otherQuery));
  function setSort(v: string) {
    ui.otherSort = v as OtherSort;
  }

  /** Search and the Beta settings decide what is listed at all. */
  function listed(c: CatalogEntry): boolean {
    if (c.prerelease && c.source === "blender" && s.blenderChannel !== "beta") return false;
    if (c.prerelease && c.source === "craft" && !s.craftPrereleases) return false;
    return true;
  }
  const onSources = $derived(SECTIONS.filter((x) => x.enabled()));
  const sourcesOff = $derived(allSourcesOff(s.sources));
  // the curated sources, best matches first when searching; Other apps come from the backend
  const found = $derived.by(() => {
    // Installed apps are left out of the list, but a search always finds them (with Open).
    const base = (catalog ?? []).filter(listed).filter((c) => !c.installed || !!q || s.showInstalledInGetApps);
    return q ? rankEntries(base, q) : base;
  });
  const otherHave = $derived(found.filter((c) => c.group === "Other apps" && c.installed).length);
  const otherTotal = $derived(ui.otherBrowse?.total ?? 0);
  const counts = $derived(Object.fromEntries(SECTIONS.map((x) => [x.group, x.group === "Other apps" ? otherTotal : found.filter((c) => c.group === x.group).length])) as Record<Group, number>);
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
        // Other apps: one place per entry of the whole list, null until its page is loaded
        if (x.group === "Other apps") return { ...x, items: [] as CatalogEntry[], slots: ui.otherBrowse?.entries ?? [], total: otherTotal };
        const all = found.filter((c) => c.group === x.group);
        // what you can still install comes first; Blender keeps its own order (stable, beta, LTS, older)
        const items = x.group === "Blender" ? all : [...all.filter((c) => !c.installed), ...all.filter((c) => c.installed)];
        return { ...x, items, slots: [], total: items.length };
      })
      // Other apps stay listed: their own line says when nothing matches.
      .filter((x) => x.items.length > 0 || x.group === "Other apps"),
  );
  const shownGroups = $derived(new Set(sections.map((x) => x.group)));
  const curated = $derived(found.filter((c) => c.group !== "Other apps" && shownGroups.has(c.group as Group)));
  // Other apps hides installed ones unless the setting shows them: then they are in the total too
  const toInstall = $derived(curated.filter((c) => !c.installed).length + (shownGroups.has("Other apps") ? Math.max(0, otherTotal - (s.showInstalledInGetApps ? otherHave : 0)) : 0));
  const have = $derived(curated.filter((c) => c.installed).length + (shownGroups.has("Other apps") ? otherHave : 0));
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
        Every source is switched off. <a class="lnk" href="#/settings/apps">Turn some on in Settings, Apps</a>.
      {:else}
        {q ? `No apps match "${ui.getQuery}".` : "Nothing to show for this source."}
      {/if}
    </p>
  {/if}

  {#if catalog}
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
        {@const b = ui.otherBrowse}
        <div class="otools">
          <div class="chips" role="group" aria-label="Category">
            <button class="chip" class:on={!ui.otherCategory} aria-pressed={!ui.otherCategory} onclick={() => (ui.otherCategory = null)}>All</button>
            {#each b?.categories ?? [] as k (k.name)}
              {#if k.count > 0 || ui.otherCategory === k.name}
                <button class="chip" class:on={ui.otherCategory === k.name} aria-pressed={ui.otherCategory === k.name} onclick={() => (ui.otherCategory = k.name)}>{k.name}<span class="n">{k.count.toLocaleString("en-US")}</span></button>
              {/if}
            {/each}
          </div>
          <Select options={sortOpts} value={ui.otherSort} onchange={setSort} width={170} height={28} label="Sort Other apps" title="Sort" />
        </div>
        <div class="letters" role="group" aria-label="First letter" bind:this={letterRow}>
          <button class="let" class:on={!ui.otherLetter} aria-pressed={!ui.otherLetter} onclick={() => (ui.otherLetter = null)}>All</button>
          {#each LETTERS as l (l)}
            <button class="let" class:on={ui.otherLetter === l} aria-pressed={ui.otherLetter === l} title={l === "#" ? "Names that do not start with A to Z" : `Names that start with ${l}`} onclick={() => (ui.otherLetter = l)}>{l}</button>
          {/each}
        </div>
        {#if ui.otherSort === "updated" && b && browseCurrent}<p class="srcnote">{datedNote(b.dated, b.total)}</p>{/if}
        <div class:stale={!browseCurrent && browseBusy}>
          <VirtualGrid items={sec.slots} key={(c) => c.id} onrange={onRange} overscan={store.status.machineTier === "low" ? 1 : store.status.machineTier === "high" ? 3 : 2}>
            {#snippet cell(c: CatalogEntry, i: number)}
              <div class="slot" use:enterOnce={{ id: c.id, i }}>
                <CatalogCard {c} eager />
              </div>
            {/snippet}
            {#snippet placeholder()}
              <div class="card skel" aria-hidden="true"><Paint kind="blob" fill="#1c1c1c" /></div>
            {/snippet}
          </VirtualGrid>
        </div>
        {#if browseBusy && !sec.slots.length}
          <div class="grid" aria-hidden="true">
            {#each Array(8) as _, i (i)}<div class="card skel" style="animation-delay:{i * 80}ms"><Paint kind="blob" fill="#1c1c1c" /></div>{/each}
          </div>
        {/if}
        {#if browseError}
          <p class="none">Could not load more apps. {browseError} <button class="more" onclick={() => (ui.otherBrowse && browseCurrent ? ((browseError = null), fill()) : loadBrowse(otherKey))}>Try again</button></p>
        {:else if !browseBusy && browseCurrent && b && b.total === 0}
          <p class="none">{otherQuery ? `No Other apps match "${ui.getQuery}"${b.category || b.letter ? " with these filters" : ""}.` : b.letter ? `No apps here start with ${b.letter}.` : "No apps in this category."}</p>
        {/if}
      {:else}
        <div class="grid">
          {#each sec.items as c, i (c.id)}
            <div class="slot" animate:flip={{ duration: reduced() ? 0 : 220 }} use:enter={i}>
              <CatalogCard {c} eager={si === 0 && i < 8} />
            </div>
          {/each}
        </div>
      {/if}
    {/each}
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
  .otools {
    margin-top: 12px;
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }
  .otools .chips {
    margin-top: 0;
  }
  .stale {
    opacity: 0.6;
    transition: opacity var(--t-fast) ease;
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
