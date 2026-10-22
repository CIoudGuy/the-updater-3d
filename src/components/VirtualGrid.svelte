<script lang="ts" generics="T">
  // A card grid for long lists: only the rows near the visible part of the page are in the DOM,
  // the rest is empty space of the same height, so thousands of entries scroll as fast as fifty.
  // Every row has the same height, so the whole list has its full height from the start (the
  // scrollbar can be dragged anywhere) and nothing moves when cards or their pictures arrive.
  // A place whose entry is not loaded yet (null) shows the placeholder, of the same size.
  import { onMount, untrack, type Snippet } from "svelte";
  import { windowRows } from "../lib/browse";

  let {
    items,
    key,
    cell,
    placeholder,
    rowHeight = 168,
    gap = 16,
    overscan = 2,
    onrange,
  }: {
    /** null: a place whose item is not loaded yet */
    items: T[];
    key: (item: NonNullable<T>) => string;
    /** one card; the index is the item's place in the whole list */
    cell: Snippet<[NonNullable<T>, number]>;
    /** a place whose item is not loaded yet */
    placeholder?: Snippet<[number]>;
    /** height of a card; the grid never measures it */
    rowHeight?: number;
    gap?: number;
    /** screens of rows kept above and below the visible ones, so a quick fling never shows a gap */
    overscan?: number;
    /** the places in view (`from` to `to`) and the ones kept around them (`keepFrom` to `keepTo`) */
    onrange?: (r: { from: number; to: number; keepFrom: number; keepTo: number }) => void;
  } = $props();

  let box = $state<HTMLDivElement>();
  let grid = $state<HTMLDivElement>();
  let cols = $state(4);
  let first = $state(0);
  let last = $state(0);

  const rowH = $derived(rowHeight + gap);
  const rows = $derived(Math.ceil(items.length / cols));
  const top = $derived(first * rowH);
  const bottom = $derived(Math.max(0, rows - last) * rowH);
  const shown = $derived.by(() => {
    const out: number[] = [];
    for (let i = first * cols; i < Math.min(last * cols, items.length); i++) out.push(i);
    return out;
  });
  const slotKey = (i: number) => {
    const it = items[i];
    return it == null ? `#${i}` : key(it);
  };

  let scrollEl: HTMLElement | null = null;
  function scroller(): HTMLElement | null {
    let p = box?.parentElement ?? null;
    while (p) {
      const o = getComputedStyle(p).overflowY;
      if (o === "auto" || o === "scroll") return p;
      p = p.parentElement;
    }
    return null;
  }

  function measureCols() {
    if (!grid) return;
    const n = getComputedStyle(grid).gridTemplateColumns.split(" ").filter(Boolean).length;
    if (n > 0 && n !== cols) cols = n;
  }

  let sent = "";
  /** Runs in the scroll event itself, so the new rows are in the same frame as the scroll. */
  function update() {
    if (!box) return;
    const s = scrollEl;
    const viewTop = s ? s.getBoundingClientRect().top : 0;
    const viewH = s ? s.clientHeight : window.innerHeight;
    const above = viewTop - box.getBoundingClientRect().top;
    const w = windowRows(above, viewH, rowH, rows, overscan * viewH);
    if (w.first !== first) first = w.first;
    if (w.last !== last) last = w.last;
    if (onrange) {
      const v = windowRows(above, viewH, rowH, rows, 0);
      const r = { from: v.first * cols, to: Math.min(items.length, v.last * cols), keepFrom: w.first * cols, keepTo: Math.min(items.length, w.last * cols) };
      const k = `${r.from} ${r.to} ${r.keepFrom} ${r.keepTo} ${items.length}`;
      if (k !== sent) {
        sent = k;
        onrange(r);
      }
    }
  }

  onMount(() => {
    scrollEl = scroller();
    const s = scrollEl ?? window;
    s.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(() => {
      measureCols();
      update();
    });
    if (box) ro.observe(box);
    if (scrollEl) ro.observe(scrollEl);
    measureCols();
    update();
    return () => {
      s.removeEventListener("scroll", update);
      ro.disconnect();
    };
  });

  // a new list, or one that got longer or shorter: the rows in view may change
  $effect(() => {
    items.length;
    cols;
    untrack(update);
  });
</script>

<div class="vbox" bind:this={box} style="padding-top:{top}px;padding-bottom:{bottom}px">
  <div class="vgrid" bind:this={grid} style="grid-auto-rows:{rowHeight}px;row-gap:{gap}px">
    {#each shown as i (slotKey(i))}
      {@const it = items[i]}
      {#if it != null}
        {@render cell(it, i)}
      {:else if placeholder}
        {@render placeholder(i)}
      {:else}
        <div></div>
      {/if}
    {/each}
  </div>
</div>

<style>
  .vbox {
    margin-top: 14px;
    /* rows come and go above the view while scrolling: the browser must not try to keep them in place */
    overflow-anchor: none;
  }
  .vgrid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    column-gap: 16px;
    contain: layout style;
  }
  @media (max-width: 999px) {
    .vgrid {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
  }
</style>
