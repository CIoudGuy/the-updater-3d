<script lang="ts">
  import { Tween } from "svelte/motion";
  import { cubicOut } from "svelte/easing";
  import { BRUSH } from "../lib/paths";
  import { reduced } from "../lib/motion";

  let {
    value,
    color = "#EDEDED",
    opacity = 0.85,
    height = 6,
    track = "#232323",
    shine = true,
    fillOpacityOverride = null,
  }: { value: number; color?: string; opacity?: number; height?: number; track?: string; shine?: boolean; fillOpacityOverride?: number | null } = $props();

  const t = Tween.of(() => Math.max(0, Math.min(1, value)), { duration: reduced() ? 0 : 450, easing: cubicOut });
</script>

<span class="bar" style="height:{height}px" aria-hidden="true">
  <svg viewBox="0 0 400 24" preserveAspectRatio="none"><path d={BRUSH} fill={track} /></svg>
  <svg class="fill" viewBox="0 0 400 24" preserveAspectRatio="none" style="clip-path: inset(0 {(1 - t.current) * 100}% 0 0)">
    <path d={BRUSH} fill={color} fill-opacity={fillOpacityOverride ?? opacity} />
    {#if shine}<path d="M14 9c90-2 240-2.4 370-.4" fill="none" stroke="#FFFFFF" stroke-opacity="0.3" stroke-width="1" stroke-linecap="round" vector-effect="non-scaling-stroke" />{/if}
  </svg>
</span>

<style>
  .bar {
    position: relative;
    display: block;
    width: 100%;
  }
  svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    overflow: visible;
  }
  .fill path:first-child {
    transition: fill-opacity 0.3s ease;
  }
</style>
