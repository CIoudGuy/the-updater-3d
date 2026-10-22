<script lang="ts">
  import { SPLASH, SPLASH_LINES } from "../lib/paths";

  // Decorative paint blobs in the corner. Colours come from the apps that need you.
  let { colors, opacity = 0.08, lines = true, left = false }: { colors: string[]; opacity?: number; lines?: boolean; left?: boolean } = $props();
  const c = $derived([colors[0] ?? "#A0A0A0", colors[1] ?? colors[0] ?? "#A0A0A0", colors[2] ?? colors[0] ?? "#A0A0A0"]);
</script>

<svg class="splash" class:left viewBox="0 0 300 300" aria-hidden="true">
  {#if left}
    <g class="drift">
      <path d={SPLASH[1]} fill={c[0]} fill-opacity={opacity} />
      <path d="M50 120c60-20 140-16 210 10M70 150c50-12 120-8 170 14M90 180c40-6 90 0 130 14" fill="none" stroke={c[0]} stroke-opacity="0.1" stroke-width="1.4" stroke-linecap="round" />
    </g>
  {:else}
  <g class="drift">
    <path d={SPLASH[0]} transform="translate(70 -6) scale(0.72)" fill={c[0]} fill-opacity={opacity + 0.01} />
    <path d={SPLASH[1]} transform="translate(10 70) scale(0.66)" fill={c[1]} fill-opacity={opacity} />
    <path d={SPLASH[2]} transform="translate(120 130) scale(0.5)" fill={c[2]} fill-opacity={opacity} />
    {#if lines}<path d={SPLASH_LINES} fill="none" stroke="#EDEDED" stroke-opacity="0.05" stroke-width="1.4" stroke-linecap="round" />{/if}
  </g>
  {/if}
</svg>

<style>
  .splash {
    position: absolute;
    right: -70px;
    top: -30px;
    width: 460px;
    height: 460px;
    pointer-events: none;
    overflow: visible;
  }
  .splash.left {
    right: auto;
    left: 20px;
    top: -120px;
    width: 520px;
    height: 380px;
  }
  path {
    transition: fill 0.5s ease;
  }
  .drift {
    transform-origin: 150px 150px;
    animation: drift 22s ease-in-out infinite alternate;
  }
  @keyframes drift {
    from {
      transform: translate(0, 0) rotate(0deg);
    }
    to {
      transform: translate(-6px, 8px) rotate(2.5deg);
    }
  }
  @media (max-width: 999px) {
    .splash {
      width: 340px;
      height: 340px;
      right: -60px;
    }
  }
</style>
