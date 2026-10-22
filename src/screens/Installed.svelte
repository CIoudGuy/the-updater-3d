<script lang="ts">
  import { flip } from "svelte/animate";
  import { onMount } from "svelte";
  import Segmented from "../components/Segmented.svelte";
  import Select from "../components/Select.svelte";
  import Tile from "../components/Tile.svelte";
  import Row from "../components/Row.svelte";
  import Icon from "../components/Icon.svelte";
  import Num from "../components/Num.svelte";
  import { I } from "../lib/icons";
  import { GROUPS, store } from "../lib/store.svelte";
  import { ui, type InstalledFilter, type InstalledGroup, type SortKey } from "../lib/ui.svelte";
  import { enter, reduced, rise } from "../lib/motion";
  import type { AppEntry, AppStatus } from "../lib/types";
  import { groupOrAll, installedNone } from "../lib/view";

  let searchInput = $state<HTMLInputElement>();
  onMount(() => {
    ui.searchEl = searchInput ?? null;
    return () => {
      if (ui.searchEl === searchInput) ui.searchEl = null;
    };
  });

  const RECENT_MS = 14 * 86400e3;
  const STATUS_RANK: Record<AppStatus, number> = { failed: 0, missing: 1, blocked: 2, update: 3, updating: 4, queued: 5, unknown: 6, current: 7 };

  const q = $derived(ui.query.trim().toLowerCase());
  // a source filter whose last app was uninstalled falls back to All, like in Get apps
  const presentGroups = $derived(GROUPS.filter((g) => store.apps.some((a) => a.group === g)));
  const group = $derived(groupOrAll(ui.group, presentGroups));
  const scoped = $derived(
    store.apps.filter(
      (a) =>
        (group === "all" || a.group === group) &&
        (!q || a.name.toLowerCase().includes(q) || a.version.toLowerCase().includes(q) || a.code.toLowerCase() === q || a.group.toLowerCase().includes(q)),
    ),
  );
  const filtered = $derived(
    scoped.filter((a) => {
      if (ui.filter === "updates") return a.status === "update";
      if (ui.filter === "recent") return !!a.updatedAt && Date.now() - +new Date(a.updatedAt) < RECENT_MS;
      return true;
    }),
  );
  const sorted = $derived.by(() => {
    const key: SortKey = ui.filter === "recent" ? "updated" : ui.sort;
    const list = [...filtered];
    const byName = (a: AppEntry, b: AppEntry) => a.name.localeCompare(b.name, undefined, { numeric: true });
    list.sort((a, b) => {
      switch (key) {
        case "size":
          return (b.sizeOnDisk ?? 0) - (a.sizeOnDisk ?? 0) || byName(a, b);
        case "updated":
          return +new Date(b.updatedAt ?? 0) - +new Date(a.updatedAt ?? 0) || byName(a, b);
        case "status":
          return STATUS_RANK[a.status] - STATUS_RANK[b.status] || byName(a, b);
        default:
          return byName(a, b);
      }
    });
    return list;
  });
  const showSections = $derived(ui.view === "grid" && ui.filter === "all" && group === "all" && sorted.length > 0);
  const sections = $derived(
    GROUPS.map((g) => ({ name: g, items: sorted.filter((a) => a.group === g) })).filter((s) => s.items.length > 0),
  );
  const none = $derived(installedNone({ query: ui.query, filter: ui.filter, matchesIgnoringFilter: scoped.length }));
  const updatesCount = $derived(store.updates.length);

  const filterOpts = $derived([
    { value: "updates" as InstalledFilter, label: ui.compact ? "Updates" : "Updates available", badge: updatesCount || "", badgeColor: "var(--warn)" },
    { value: "all" as InstalledFilter, label: "All", badge: store.apps.length },
    { value: "recent" as InstalledFilter, label: ui.compact ? "Recent" : "Recently updated" },
  ]);
  const groupOpts = $derived([{ value: "all" as InstalledGroup, label: "All sources" }, ...presentGroups.map((g) => ({ value: g as InstalledGroup, label: g }))]);
  const viewOpts = [
    { value: "grid" as const, icon: I.installed, title: "Grid view  G" },
    { value: "list" as const, icon: I.list, title: "List view  L" },
  ];
  const sortOpts = [
    { value: "name", label: "Sort by name" },
    { value: "size", label: "Sort by size" },
    { value: "updated", label: "Sort by last updated" },
    { value: "status", label: "Sort by status" },
  ];
</script>

<section class="screen installed">
  <div class="head">
    <h1 class="h1">Installed <span class="count"><Num value={store.apps.length} /></span></h1>
    <div class="tools">
      <label class="search">
        <Icon d={I.search} />
        <input bind:this={searchInput} bind:value={ui.query} aria-label="Search apps" placeholder="Search apps" spellcheck="false" autocomplete="off" />
        {#if !ui.compact}<span class="kbd">Ctrl F</span>{/if}
      </label>
      <Segmented variant="icon" options={viewOpts} value={ui.view} onchange={(v) => ui.setView(v)} label="View" />
    </div>
  </div>

  <div class="filters">
    <div class="chips">
      <Segmented variant="chip" options={filterOpts} value={ui.filter} onchange={(v) => (ui.filter = v)} label="Filter" />
      {#if presentGroups.length > 1}
        <span class="vsep" aria-hidden="true"></span>
        <Segmented variant="chip" options={groupOpts} value={group} onchange={(v) => (ui.group = v)} label="Source" />
      {/if}
    </div>
    <Select options={sortOpts} value={ui.sort} onchange={(v) => ui.setSort(v as SortKey)} width={ui.compact ? 150 : 190} height={30} label="Sort" title="Sort" />
  </div>

  {#if sorted.length === 0}
    <p class="none" in:rise>
      {none.text}
      {#if none.showAll}<button class="lnk" onclick={() => (ui.filter = "all")}>Show all apps</button>{/if}
    </p>
  {:else if ui.view === "grid"}
    {#if showSections}
      {#each sections as sec (sec.name)}
        <h2 class="grp"><span>{sec.name}</span><span class="n">{sec.items.length}</span></h2>
        <div class="grid">
          {#each sec.items as a, i (a.id)}
            <div class="cell" animate:flip={{ duration: reduced() ? 0 : 240 }} use:enter={i}><Tile app={a} compact={ui.compact} /></div>
          {/each}
        </div>
      {/each}
    {:else}
      <div class="grid first">
        {#each sorted as a, i (a.id)}
          <div class="cell" animate:flip={{ duration: reduced() ? 0 : 240 }} use:enter={i}><Tile app={a} compact={ui.compact} /></div>
        {/each}
      </div>
    {/if}
  {:else}
    <div class="table" role="table" aria-label="Installed apps">
      <div class="th" role="row">
        <span role="columnheader">App</span><span role="columnheader">Installed</span><span role="columnheader">Latest</span><span role="columnheader">Status</span><span role="columnheader" class="r">Size</span><span role="columnheader">Updated</span><span role="columnheader" class="r">Actions</span>
      </div>
      {#each sorted as a, i (a.id)}
        <div animate:flip={{ duration: reduced() ? 0 : 240 }} use:enter={{ i, y: 6 }}><Row app={a} /></div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .installed {
    --cols: minmax(0, 1fr) 120px 120px 150px 84px 90px 130px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
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
    transition: background-color var(--t-fast) ease;
  }
  .search:hover,
  .search:focus-within {
    background: var(--surface-2);
  }
  .search:focus-within {
    color: var(--text-2);
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
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px 16px;
    margin-top: 24px;
  }
  .chips {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 4px;
  }
  .vsep {
    width: 1px;
    height: 16px;
    margin: 0 12px;
    background: var(--sep-raised);
  }
  .grp {
    margin-top: 28px;
    height: 24px;
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-2);
  }
  .grp .n {
    font-weight: 400;
    color: var(--text-3);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 16px;
    margin-top: 12px;
  }
  .grid.first {
    margin-top: 24px;
  }
  .cell {
    min-width: 0;
  }
  .none {
    margin-top: 48px;
    font-size: 13px;
    color: var(--text-3);
  }
  .none .lnk {
    margin-left: 6px;
    color: var(--text);
    text-decoration: underline;
    text-decoration-color: var(--text-4);
    text-underline-offset: 3px;
  }
  .none .lnk:hover {
    text-decoration-color: var(--text);
  }
  .table {
    margin-top: 20px;
  }
  .th {
    height: 32px;
    display: grid;
    grid-template-columns: var(--cols);
    align-items: center;
    font-size: 12px;
    color: var(--text-3);
  }
  .th .r {
    text-align: right;
    padding-right: 16px;
  }
  .th span:last-child {
    padding-right: 0;
    text-align: right;
  }
  @media (max-width: 999px) {
    .screen {
      padding-top: 28px;
    }
    .grid {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
    .search {
      width: 200px;
    }
    .installed {
      --cols: minmax(0, 1fr) 96px 96px 130px 120px;
    }
    .th span:nth-child(5),
    .th span:nth-child(6) {
      display: none;
    }
    .installed :global(.row .right),
    .installed :global(.row .dt) {
      display: none;
    }
  }
</style>
