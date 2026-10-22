<script lang="ts">
  // One version's change list. Lines starting with "# ", "## " or "### " are section headings
  // (biggest first), every other line is a change. Long lists open in place.
  import { DAB } from "../lib/paths";

  let { notes, collapsedAt = 12 }: { notes: string[]; collapsedAt?: number } = $props();

  type Row = { kind: "h"; level: 1 | 2 | 3; text: string } | { kind: "item"; text: string };
  const rows = $derived<Row[]>(
    notes.map((n) => {
      const m = /^(#{1,3}) (.+)$/.exec(n);
      return m ? { kind: "h", level: m[1].length as 1 | 2 | 3, text: m[2] } : { kind: "item", text: n };
    }),
  );
  const changes = $derived(rows.filter((r) => r.kind === "item").length);
  // A little slack, so a list is never cut just to hide one or two lines.
  const long = $derived(rows.length > collapsedAt + 3);
  let open = $state(false);
  const shown = $derived(long && !open ? rows.slice(0, collapsedAt) : rows);
</script>

<div class="nl selectable">
  {#each shown as r, i (i)}
    {#if r.kind === "h"}
      <p class="h h{r.level}" class:first={i === 0}>{r.text}</p>
    {:else}
      <p class="it"><svg viewBox="0 0 20 20" width="5" height="5" aria-hidden="true"><path d={DAB} fill="#6E6E6E" /></svg><span>{r.text}</span></p>
    {/if}
  {/each}
  {#if long}
    <button class="more" onclick={() => (open = !open)}>{open ? "Show less" : `Show all ${changes} changes`}</button>
  {/if}
</div>

<style>
  .nl {
    display: flex;
    flex-direction: column;
    gap: 4px;
    cursor: text;
  }
  .it {
    margin: 0;
    display: flex;
    gap: 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-2);
  }
  .it svg {
    flex: none;
    margin-top: 8px;
  }
  .h {
    margin: 10px 0 2px;
    line-height: 1.35;
  }
  .h.first {
    margin-top: 0;
  }
  .h1 {
    margin-top: 16px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }
  .h2 {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .h3 {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-3);
  }
  .more {
    align-self: flex-start;
    margin-top: 6px;
    padding: 0;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
    cursor: pointer;
    transition: color var(--t-fast) ease;
  }
  .more:hover,
  .more:focus-visible {
    color: var(--text);
  }
</style>
