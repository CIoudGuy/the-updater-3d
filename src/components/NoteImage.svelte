<script lang="ts">
  // A screenshot in a change list. The backend reads it from GitHub and keeps it on this PC (the
  // window loads nothing from other sites); it is asked for once the picture is near the screen.
  // A picture that can not be read shows nothing at all, never a broken image.
  import { call } from "../lib/api";

  let { url }: { url: string } = $props();

  let src = $state<string | null>(null);
  let failed = $state(false);
  let shown = $state(false);

  function near(node: HTMLElement) {
    let asked = false;
    const ask = () => {
      if (asked) return;
      asked = true;
      call("note_image", { url })
        .then((s) => {
          if (s) src = s;
          else failed = true;
        })
        .catch(() => (failed = true));
    };
    if (typeof IntersectionObserver === "undefined") {
      ask();
      return;
    }
    const io = new IntersectionObserver((es) => {
      if (es.some((e) => e.isIntersecting)) {
        ask();
        io.disconnect();
      }
    }, { rootMargin: "400px" });
    io.observe(node);
    return { destroy: () => io.disconnect() };
  }
</script>

{#if !failed}
  <span class="pic" use:near>
    {#if src}
      <img class:on={shown} {src} alt="" loading="lazy" decoding="async" onload={() => (shown = true)} onerror={() => (failed = true)} />
    {/if}
  </span>
{/if}

<style>
  .pic {
    display: block;
    margin: 4px 0 8px 15px;
    max-width: calc(100% - 15px);
  }
  img {
    display: block;
    max-width: 100%;
    max-height: 340px;
    width: auto;
    height: auto;
    border-radius: 8px;
    border: 1px solid var(--sep);
    opacity: 0;
    transition: opacity var(--t-fast) ease;
  }
  img.on {
    opacity: 1;
  }
</style>
