<script lang="ts">
  import { call } from "../lib/api";
  import { router, getHref } from "../lib/router.svelte";
  import type { OtherBuild } from "../lib/types";

  /** An Other app or catalog package: its beta, preview and nightly packages, which install as apps of their own */
  let { id }: { id: string } = $props();

  let builds = $state<OtherBuild[]>([]);
  let seq = 0;
  $effect(() => {
    const n = ++seq;
    builds = [];
    call("other_builds", { id })
      .then((r) => {
        if (n === seq) builds = r;
      })
      .catch(() => {});
  });
</script>

{#if builds.length}
  <div class="ob">
    <span class="lbl">Other builds</span>
    <div class="list">
      {#each builds as b (b.id)}
        <a class="ln" href={getHref(b.id)} title="Open {b.name} in Get apps" onclick={(e) => { e.preventDefault(); router.go(getHref(b.id)); }}>{b.name}</a>
      {/each}
    </div>
    <span class="hint">Each installs as its own app, next to this one.</span>
  </div>
{/if}

<style>
  .ob {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .lbl,
  .hint {
    font-size: 12px;
    color: var(--text-3);
  }
  .hint {
    line-height: 1.5;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: flex-start;
  }
  .ln {
    font-size: 13px;
    color: var(--text-2);
    transition: color var(--t-fast);
  }
  .ln:hover {
    color: var(--text);
  }
</style>
