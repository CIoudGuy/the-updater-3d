<script lang="ts">
  import type { Snippet } from "svelte";
  import Paint from "./Paint.svelte";
  import { BTN_BRISTLE, BTN_DANGER, BTN_HI } from "../lib/paths";

  type Variant = "primary" | "secondary" | "ghost" | "danger" | "raised";
  let {
    variant = "secondary",
    size = "lg",
    hint = "",
    href = "",
    title = "",
    label = "",
    disabled = false,
    onclick,
    children,
    class: cls = "",
  }: {
    variant?: Variant;
    size?: "lg" | "md" | "sm";
    hint?: string;
    href?: string;
    title?: string;
    label?: string;
    disabled?: boolean;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
    class?: string;
  } = $props();
</script>

<svelte:element
  this={href ? "a" : "button"}
  class="btn {variant} {size} {cls}"
  {href}
  {title}
  aria-label={label || undefined}
  disabled={href ? undefined : disabled}
  role={href ? "button" : undefined}
  {onclick}
>
  {#if variant === "primary"}
    <Paint kind="btn" fill="#EDEDED" lines={BTN_HI} lineStroke="#FFFFFF" lineWidth={1} />
  {:else if variant === "secondary"}
    <Paint kind="btn" fill="#262626" lines={BTN_BRISTLE} lineStroke="#303030" />
  {:else if variant === "danger"}
    <Paint kind="btn" fill="#2E2E2E" lines={BTN_DANGER} lineStroke="#D9706A" lineOpacity={0.6} lineWidth={1.4} />
  {/if}
  <span class="lbl">{@render children()}</span>
  {#if hint}<span class="hint">{hint}</span>{/if}
</svelte:element>

<style>
  .btn {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 500;
    border-radius: var(--r-ctl);
    transition: transform var(--t-fast) var(--ease), background-color var(--t-fast) ease, color var(--t-fast) ease;
    user-select: none;
  }
  .btn:active:not(:disabled) {
    transform: scale(0.98);
  }
  .lg {
    height: 36px;
    padding: 0 18px;
  }
  .md {
    height: 32px;
    padding: 0 16px;
  }
  .sm {
    height: 28px;
    padding: 0 12px;
    font-size: 12px;
    gap: 6px;
  }
  .lbl,
  .hint {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .hint {
    font-size: 12px;
    font-weight: 500;
  }
  .primary {
    color: var(--on-primary);
    font-weight: 600;
  }
  .primary .hint {
    color: var(--on-primary-2);
  }
  .primary:hover:not(:disabled) :global(.paint path:first-child) {
    fill: var(--primary-hover);
  }
  .secondary {
    color: var(--text);
  }
  .secondary .hint {
    color: var(--text-3);
  }
  .secondary:hover:not(:disabled) :global(.paint path:first-child) {
    fill: #2e2e2e;
  }
  .danger {
    color: var(--err);
    font-weight: 600;
  }
  .danger:hover:not(:disabled) :global(.paint path:first-child) {
    fill: #343434;
  }
  .ghost {
    color: var(--text-2);
  }
  .ghost .hint {
    color: var(--text-3);
  }
  .ghost:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface);
  }
  .raised {
    background: var(--raised);
    color: var(--text);
  }
  .raised .hint {
    color: var(--text-3);
  }
  .raised:hover:not(:disabled) {
    background: var(--raised-2);
  }
  .btn:disabled {
    opacity: 0.5;
  }
  .sm.ghost {
    padding: 0 10px;
  }
</style>
