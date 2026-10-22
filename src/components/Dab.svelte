<script lang="ts">
  import { DAB } from "../lib/paths";
  let { color, size = 8, pulse = false }: { color: string; size?: number; pulse?: boolean } = $props();
</script>

<svg class="dab" class:pulse viewBox="0 0 20 20" width={size} height={size} aria-hidden="true"><path d={DAB} fill={color} /></svg>

<style>
  .dab path {
    transition: fill 0.2s ease;
  }
  .pulse {
    animation: pulse 1.6s ease-in-out infinite;
  }
</style>
