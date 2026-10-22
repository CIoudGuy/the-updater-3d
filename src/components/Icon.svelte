<script lang="ts">
  let { d, size = 16, width = 2.25, class: cls = "" }: { d: string; size?: number; width?: number; class?: string } = $props();
</script>

<svg class={cls} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width={width} stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  <path {d} />
</svg>
