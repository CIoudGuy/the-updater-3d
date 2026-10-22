<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import { I } from "../lib/icons";
  import { ui } from "../lib/ui.svelte";

  // Split button: the main part runs the default action, the chevron opens a list of alternatives.
  let {
    title = "",
    menuLabel,
    onmain,
    onmenu,
    children,
  }: {
    title?: string;
    menuLabel: string;
    onmain: () => void;
    /** called with the chevron so the list can anchor to it */
    onmenu: (anchor: HTMLButtonElement) => void;
    children: Snippet;
  } = $props();

  let chev = $state<HTMLButtonElement>();
  const open = $derived(!!chev && ui.popover?.anchor === chev);
</script>

<span class="split">
  <button class="main" {title} onclick={onmain}>{@render children()}</button>
  <button
    bind:this={chev}
    class="chev"
    class:open
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label={menuLabel}
    title={menuLabel}
    onclick={() => chev && onmenu(chev)}
  >
    <Icon d={I.chevronDown} size={14} width={2.5} />
  </button>
</span>

<style>
  .split {
    display: inline-flex;
    gap: 2px;
    flex: none;
  }
  .main,
  .chev {
    height: 28px;
    background: var(--raised);
    color: var(--text);
    font-size: 12px;
    font-weight: 500;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background-color var(--t-fast) ease, transform var(--t-fast) var(--ease);
  }
  .main {
    gap: 6px;
    padding: 0 12px;
    border-radius: 6px 2px 2px 6px;
  }
  .chev {
    width: 26px;
    border-radius: 2px 6px 6px 2px;
    color: var(--text-2);
  }
  .main:hover,
  .chev:hover,
  .chev.open {
    background: var(--raised-2);
  }
  .chev:hover,
  .chev.open {
    color: var(--text);
  }
  .main:active,
  .chev:active {
    transform: scale(0.97);
  }
  .chev :global(svg) {
    transition: transform 0.15s var(--ease);
  }
  .chev.open :global(svg) {
    transform: rotate(180deg);
  }
</style>
