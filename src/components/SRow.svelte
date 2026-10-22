<script lang="ts">
  import type { Snippet } from "svelte";

  // One settings row: label and help on the left, control on the right. Separated by a 1px line, no card.
  let {
    label,
    desc = "",
    id = "",
    first = false,
    dim = false,
    control,
    below,
  }: { label: string; desc?: string; id?: string; first?: boolean; dim?: boolean; control?: Snippet; below?: Snippet } = $props();
</script>

<div class="srow" class:first class:dim>
  <div class="main">
    <div class="txt">
      <span class="l" {id}>{label}</span>
      {#if desc}<span class="d">{desc}</span>{/if}
    </div>
    {#if control}<div class="ctl">{@render control()}</div>{/if}
  </div>
  {#if below}<div class="below">{@render below()}</div>{/if}
</div>

<style>
  .srow {
    padding: 14px 0;
    border-top: 1px solid var(--sep);
  }
  .main {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 32px;
    align-items: center;
  }
  .txt {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .l {
    font-size: 13px;
    font-weight: 500;
  }
  .d {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-3);
  }
  .ctl {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .dim .l,
  .dim .d {
    opacity: 0.5;
  }
  .below {
    margin-top: 12px;
  }
</style>
