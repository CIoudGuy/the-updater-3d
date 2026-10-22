<script lang="ts">
  import { Tween } from "svelte/motion";
  import { cubicOut } from "svelte/easing";
  import { reduced } from "../lib/motion";

  let {
    value,
    fmt = (n: number) => String(Math.round(n)),
  }: { value: number; fmt?: (n: number) => string } = $props();

  const t = Tween.of(() => value, { duration: reduced() ? 0 : 320, easing: cubicOut });
</script>

<span class="num">{fmt(t.current)}</span>

<style>
  .num {
    font-variant-numeric: tabular-nums;
  }
</style>
