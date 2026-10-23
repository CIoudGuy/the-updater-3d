<script lang="ts" generics="V extends string">
  import { tick } from "svelte";
  import Paint from "./Paint.svelte";
  import Icon from "./Icon.svelte";
  import { MARKER_BASE, place } from "../lib/motion";

  interface Opt {
    value: V;
    label?: string;
    icon?: string;
    badge?: string | number;
    badgeColor?: string;
    title?: string;
  }
  let {
    options,
    value,
    onchange,
    variant = "ctl",
    label = "",
  }: { options: Opt[]; value: V; onchange: (v: V) => void; variant?: "chip" | "ctl" | "icon"; label?: string } = $props();

  let box = $state<HTMLElement>();
  let marker = $state<HTMLElement>();
  let btns: Record<string, HTMLElement | undefined> = {};
  let anim = $state(false);

  const swatch = $derived(variant === "chip" ? "#262626" : variant === "icon" ? "#2E2E2E" : "#323232");

  function layout() {
    if (marker) place(marker, btns[value]);
  }
  $effect(() => {
    value;
    options.length;
    tick().then(layout);
  });
  $effect(() => {
    if (!box) return;
    const ro = new ResizeObserver(layout);
    ro.observe(box);
    for (const b of Object.values(btns)) if (b) ro.observe(b);
    const id = requestAnimationFrame(() => (anim = true));
    return () => {
      ro.disconnect();
      cancelAnimationFrame(id);
    };
  });
</script>

<div class="seg {variant}" bind:this={box} role="radiogroup" aria-label={label || undefined}>
  <span class="marker" class:anim bind:this={marker} style="width:{MARKER_BASE}px">
    <Paint kind="swatch" fill={swatch} />
  </span>
  {#each options as o (o.value)}
    <button
      bind:this={btns[o.value]}
      class="opt"
      class:on={o.value === value}
      role="radio"
      aria-checked={o.value === value}
      aria-label={o.icon && o.title ? o.title : undefined}
      title={o.title}
      onclick={() => onchange(o.value)}
    >
      {#if o.icon}<Icon d={o.icon} />{/if}
      {#if o.label}<span>{o.label}</span>{/if}
      {#if o.badge !== undefined && o.badge !== ""}<span class="badge" style:color={o.badgeColor ?? "var(--text-3)"}>{typeof o.badge === "number" ? o.badge.toLocaleString("en-US") : o.badge}</span>{/if}
    </button>
  {/each}
</div>

<style>
  .seg {
    position: relative;
    display: inline-flex;
    flex: none;
  }
  .marker {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
    pointer-events: none;
    opacity: 0;
  }
  .marker.anim {
    transition: transform 0.18s var(--ease), height 0.13s var(--ease), top 0.13s var(--ease), opacity 0.13s;
  }
  .opt {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-2);
    font-weight: 500;
    white-space: nowrap;
    transition: color 0.1s ease, transform var(--t-fast) var(--ease);
  }
  .opt:hover {
    color: var(--text);
  }
  .opt:active {
    transform: scale(0.97);
  }
  .opt.on {
    color: var(--text);
  }
  .chip {
    gap: 4px;
  }
  .chip .opt {
    height: 30px;
    padding: 0 14px;
    font-size: 13px;
  }
  .ctl {
    padding: 2px;
    border-radius: 6px;
    background: var(--surface);
  }
  .ctl .opt {
    height: 28px;
    padding: 0 12px;
    font-size: 12px;
  }
  .icon {
    padding: 2px;
    border-radius: 6px;
    background: var(--surface);
  }
  .icon .opt {
    width: 32px;
    height: 28px;
    color: var(--text-3);
  }
  .icon .opt.on,
  .icon .opt:hover {
    color: var(--text);
  }
  .badge {
    font-size: 13px;
  }
  .opt.on .badge {
    color: var(--text-3);
  }
</style>
