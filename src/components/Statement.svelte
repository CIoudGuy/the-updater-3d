<script lang="ts">
  import { BRUSH, BRUSH_LINES } from "../lib/paths";

  let {
    pre = "",
    word,
    post = "",
    color,
    opacity = 0.5,
    size = 64,
  }: { pre?: string; word: string; post?: string; color: string; opacity?: number; size?: number } = $props();
</script>

<h1 style="font-size:{size}px" class="statement">
  {pre}<span class="w"
    >{#key word + color}<svg class="under" viewBox="0 0 400 24" preserveAspectRatio="none" aria-hidden="true"
        ><path d={BRUSH} fill={color} fill-opacity={opacity} /><path d={BRUSH_LINES} fill="none" stroke={color} stroke-opacity="0.6" stroke-width="0.9" stroke-linecap="round" /></svg
      >{/key}<span class="t">{word}</span></span
  >{post}
</h1>

<style>
  .statement {
    position: relative;
    margin: 0;
    line-height: 1;
    font-weight: 600;
    letter-spacing: var(--track-display);
  }
  .w {
    position: relative;
    display: inline-block;
  }
  .t {
    position: relative;
  }
  .under {
    position: absolute;
    left: -10px;
    bottom: -6px;
    width: calc(100% + 26px);
    height: 24px;
    overflow: visible;
    animation: draw 0.42s var(--ease) both;
  }
</style>
