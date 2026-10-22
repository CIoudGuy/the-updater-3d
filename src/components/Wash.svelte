<script lang="ts">
  import { Tween } from "svelte/motion";
  import { cubicOut } from "svelte/easing";
  import { WAVE_EDGE, WAVE_FILL } from "../lib/paths";
  import { reduced } from "../lib/motion";

  // Colour wash that rises from the bottom of a card. The edge waves slowly while active.
  let { color, level, active = false, rest = 0.08 }: { color: string; level: number; active?: boolean; rest?: number } = $props();
  const lv = Tween.of(() => level, { duration: reduced() ? 0 : 700, easing: cubicOut });
</script>

<div class="wash" aria-hidden="true">
  <div class="rise" style="transform: translateY(calc({(1 - lv.current) * 100}% - 4%))">
    <svg class="wave" class:moving={active} viewBox="0 0 400 100" preserveAspectRatio="none">
      <path class="fillp" d={WAVE_FILL} style="fill:{color};fill-opacity:{active ? 0.26 : rest}" />
      <path class="edge" class:on={active} d={WAVE_EDGE} fill="none" stroke="#EDEDED" stroke-width="2" vector-effect="non-scaling-stroke" />
    </svg>
  </div>
</div>

<style>
  .wash {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: var(--r-card);
    pointer-events: none;
    /* Own layer: without it WebView2 can paint the wash over the whole card. */
    contain: paint;
    transform: translateZ(0);
  }
  .rise {
    position: absolute;
    left: 0;
    top: 0;
    width: 100%;
    height: 100%;
    will-change: transform;
  }
  .wave {
    position: absolute;
    left: 0;
    top: 0;
    width: 200%;
    height: 100%;
  }
  .wave.moving {
    animation: wave 5.5s linear infinite;
  }
  .fillp {
    transition: fill-opacity 0.4s ease, fill 0.3s ease;
  }
  .edge {
    opacity: 0;
    transition: opacity 0.4s ease;
  }
  .edge.on {
    opacity: 0.9;
  }
</style>
