<script lang="ts">
  import Icon from "./Icon.svelte";
  import { I } from "../lib/icons";
  import { ui } from "../lib/ui.svelte";

  interface Opt {
    value: string;
    label: string;
  }
  let {
    options,
    value,
    onchange,
    width = 180,
    label = "",
    height = 32,
    title = "",
    id = "",
    display = "",
  }: { options: Opt[]; value: string; onchange: (v: string) => void; width?: number | string; label?: string; height?: number; title?: string; id?: string; display?: string } = $props();

  let btn = $state<HTMLButtonElement>();
  const open = $derived(ui.popover?.anchor === btn && !!btn);
  const current = $derived(options.find((o) => o.value === value)?.label ?? display);

  export function focus() {
    btn?.focus();
  }

  function toggle() {
    if (!btn) return;
    if (open) return ui.closePopover();
    ui.openList({
      rect: btn.getBoundingClientRect(),
      items: options.map((o) => ({ id: o.value, label: o.label })),
      selected: value,
      width: btn.offsetWidth,
      align: "start",
      label,
      anchor: btn,
      onpick: onchange,
    });
  }
</script>

<button {id} bind:this={btn} class="select" class:open aria-haspopup="listbox" aria-expanded={open} aria-label={label || undefined} {title} style="width:{typeof width === 'number' ? width + 'px' : width};height:{height}px" onclick={toggle}>
  <span class="val">{current}</span>
  <Icon d={I.chevronDown} size={height < 32 ? 12 : 14} width={2.5} class="chev" />
</button>

<style>
  .select {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 0 10px;
    border-radius: var(--r-ctl);
    background: var(--surface);
    color: var(--text);
    font-size: 13px;
    text-align: left;
    transition: background-color var(--t-fast) ease, transform var(--t-fast) var(--ease);
  }
  .select:hover,
  .select.open {
    background: var(--surface-2);
  }
  .select:active {
    transform: scale(0.99);
  }
  .val {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .select :global(.chev) {
    color: var(--text-3);
    transition: transform 0.1s var(--ease);
  }
  .open :global(.chev) {
    transform: rotate(180deg);
  }
</style>
