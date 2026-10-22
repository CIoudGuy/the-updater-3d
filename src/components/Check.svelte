<script lang="ts">
  import type { Snippet } from "svelte";
  let { checked, onchange, children }: { checked: boolean; onchange: (v: boolean) => void; children: Snippet } = $props();
</script>

<button class="check" role="checkbox" aria-checked={checked} onclick={() => onchange(!checked)}>
  <span class="box" class:on={checked}>
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="#141414" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12l5 5 9-10" /></svg>
  </span>
  <span class="text">{@render children()}</span>
</button>

<style>
  .check {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    font-size: 13px;
    color: var(--text);
  }
  .box {
    width: 16px;
    height: 16px;
    flex: none;
    border-radius: 4px;
    background: #3a3a3a;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color 0.12s ease;
  }
  .box svg {
    opacity: 0;
    transform: scale(0.6);
    transition: opacity 0.12s ease, transform 0.12s var(--ease);
  }
  .box.on {
    background: var(--primary);
  }
  .box.on svg {
    opacity: 1;
    transform: none;
  }
  .check:active .box {
    transform: scale(0.92);
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
</style>
