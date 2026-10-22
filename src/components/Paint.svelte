<script lang="ts">
  import { BLOB, BRUSH, BTN, MARK, SWATCH } from "../lib/paths";

  type Kind = "btn" | "blob" | "brush" | "swatch" | "mark";
  let {
    kind,
    fill,
    opacity = 1,
    lines = null,
    lineStroke = "",
    lineOpacity = 1,
    lineWidth = 1,
  }: { kind: Kind; fill: string; opacity?: number; lines?: string | null; lineStroke?: string; lineOpacity?: number; lineWidth?: number } = $props();

  const SHAPES: Record<Kind, [string, string]> = {
    btn: ["0 0 120 36", BTN],
    blob: ["0 0 200 200", BLOB],
    brush: ["0 0 400 24", BRUSH],
    swatch: ["0 0 200 36", SWATCH],
    mark: ["0 0 48 48", MARK],
  };
  const shape = $derived(SHAPES[kind]);
</script>

<svg class="paint" aria-hidden="true" viewBox={shape[0]} preserveAspectRatio="none">
  <path d={shape[1]} style="fill:{fill}" fill-opacity={opacity} />
  {#if lines}
    <path d={lines} fill="none" stroke={lineStroke} stroke-opacity={lineOpacity} stroke-width={lineWidth} stroke-linecap="round" vector-effect="non-scaling-stroke" />
  {/if}
</svg>

<style>
  .paint {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
    overflow: visible;
  }
  .paint path {
    transition: fill var(--t-fast) ease;
  }
</style>
