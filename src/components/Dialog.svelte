<script lang="ts">
  import type { Snippet } from "svelte";
  import { onMount } from "svelte";
  import { fade } from "svelte/transition";
  import Paint from "./Paint.svelte";
  import { pop, reduced } from "../lib/motion";

  let {
    onclose,
    onenter,
    width = 480,
    labelledby = "",
    role = "dialog",
    children,
  }: { onclose: () => void; onenter?: () => void; width?: number; labelledby?: string; role?: "dialog" | "alertdialog"; children: Snippet } = $props();

  let root = $state<HTMLElement>();
  onMount(() => {
    const prev = document.activeElement as HTMLElement | null;
    root?.focus({ preventScroll: true });
    return () => prev?.focus?.({ preventScroll: true });
  });

  function onkey(e: KeyboardEvent) {
    if (e.key === "Tab" && root) {
      const f = [...root.querySelectorAll<HTMLElement>('button:not(:disabled), [href], input, [tabindex="0"]')].filter((x) => x.offsetParent !== null);
      if (!f.length) return;
      const first = f[0];
      const last = f[f.length - 1];
      const a = document.activeElement;
      if (e.shiftKey && (a === first || a === root)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && a === last) {
        e.preventDefault();
        first.focus();
      }
    } else if (e.key === "Enter" && onenter && !(e.target instanceof HTMLButtonElement) && !(e.target instanceof HTMLAnchorElement)) {
      e.preventDefault();
      onenter();
    }
  }
</script>

<div class="scrim" role="presentation" transition:fade|global={{ duration: reduced() ? 0 : 100 }} onpointerdown={onclose}></div>
<div class="wrap">
  <div
    class="dlg"
    bind:this={root}
    {role}
    aria-modal="true"
    aria-labelledby={labelledby || undefined}
    tabindex="-1"
    style="width:{width}px"
    onkeydown={onkey}
    in:pop|global={{ duration: 120, origin: "center 40%", from: 0.96 }}
    out:pop|global={{ duration: 80, origin: "center 40%", from: 0.97 }}
  >
    <Paint kind="blob" fill="#262626" lines="M14 10c50-1.4 120-1.6 172-.4M18 190c60 1 110 .8 164-.6" lineStroke="#2E2E2E" />
    <div class="body">{@render children()}</div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    left: 0;
    right: 0;
    top: var(--topbar-h);
    bottom: 0;
    background: var(--scrim);
    z-index: 70;
  }
  .wrap {
    position: fixed;
    left: 0;
    right: 0;
    top: var(--topbar-h);
    bottom: 0;
    z-index: 71;
    display: flex;
    align-items: center;
    justify-content: center;
    padding-bottom: 56px;
    pointer-events: none;
  }
  .dlg {
    position: relative;
    max-width: calc(100vw - 48px);
    padding: 28px 28px 24px;
    pointer-events: auto;
  }
  .dlg:focus-visible {
    outline: none;
  }
  .dlg :global(.btn.raised) {
    background: #2e2e2e;
  }
  .dlg :global(.btn.raised:hover:not(:disabled)) {
    background: #343434;
  }
  .body {
    position: relative;
    display: flex;
    flex-direction: column;
  }
</style>
