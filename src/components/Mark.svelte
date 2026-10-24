<script lang="ts">
  import { MARK, MARK_LINES, MARK_LINES_BIG } from "../lib/paths";
  import { mostlyDark } from "../lib/iconlook";

  // Apps from the catalog and other programs show a soft shimmer until their real icon has loaded
  // (it is looked up in the background). Apps with a painted brand mark (Adobe, Blender, drivers)
  // show that mark, with the real icon fading in over it when there is one.
  let {
    app,
    size = 32,
    fs = 0,
    dim = false,
    eager = true,
  }: {
    app: { id?: string; code: string; fg: string; bg: string; iconUrl: string | null };
    size?: number;
    fs?: number;
    dim?: boolean;
    /** icons in the first screenful decode right away; pass false for long lists below the fold */
    eager?: boolean;
  } = $props();

  let loadedUrl = $state<string | null>(null);
  let failedUrl = $state<string | null>(null);
  const url = $derived(app.iconUrl);
  const showImg = $derived(!!url && failedUrl !== url);
  const loaded = $derived(!!url && loadedUrl === url);
  const font = $derived(fs || Math.max(9, Math.round(size * 0.4)));
  /** No painted mark for these: their icon comes from the app's own files or site. */
  const shimmer = $derived(!loaded && /^(other:|prog:|msstore:)/.test(app.id ?? ""));

  /** Cached images can finish before Svelte attaches onload, so check `complete` when the node mounts. */
  function watch(img: HTMLImageElement) {
    if (img.complete && img.naturalWidth > 0) done(img);
  }

  // A dark logo on a clear background (Spotify's black circle) is lost on the dark cards, so it
  // gets a light plate. The backend names such icons "*.dark.png"; other pictures are looked at
  // here when the browser lets the page read them.
  let darkUrl = $state<string | null>(null);
  const plate = $derived(loaded && (/\.dark\.png($|[?#])/i.test(url ?? "") || darkUrl === url));
  function done(img: HTMLImageElement) {
    loadedUrl = url;
    if (url && !/\.(dark|icon)\.png($|[?#])/i.test(url) && looksDark(img)) darkUrl = url;
  }
  function looksDark(img: HTMLImageElement): boolean {
    try {
      const n = 24;
      const cv = document.createElement("canvas");
      cv.width = cv.height = n;
      const g = cv.getContext("2d", { willReadFrequently: true });
      if (!g) return false;
      g.drawImage(img, 0, 0, n, n);
      return mostlyDark(g.getImageData(0, 0, n, n).data);
    } catch {
      // a picture from another origin cannot be read: it keeps no plate
      return false;
    }
  }
</script>

<span class="mark" class:dim class:plate style="width:{size}px;height:{size}px" aria-hidden="true">
  {#if shimmer}
    <span class="shim"></span>
  {:else}
    <svg class:gone={loaded} viewBox="0 0 48 48" width={size} height={size}>
      <path d={MARK} fill={app.bg} />
      <path d={size >= 64 ? MARK_LINES_BIG : MARK_LINES} fill="none" stroke={app.fg} stroke-opacity="0.3" stroke-width="1.6" stroke-linecap="round" />
    </svg>
    <span class="code" class:gone={loaded} style="font-size:{font}px;color:{app.fg}">{app.code}</span>
  {/if}
  {#if showImg}
    {#key url}
      <img
        class:on={loaded}
        src={url}
        alt=""
        width={size}
        height={size}
        decoding="async"
        loading={eager ? "eager" : "lazy"}
        use:watch
        onload={(e) => done(e.currentTarget as HTMLImageElement)}
        onerror={() => (failedUrl = url)}
      />
    {/key}
  {/if}
</span>

<style>
  .mark {
    position: relative;
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .mark.dim {
    opacity: 0.7;
  }
  svg {
    position: absolute;
    inset: 0;
  }
  /* once the real icon is fully in, drop the mark underneath so transparent corners stay clean */
  svg.gone {
    opacity: 0;
    transition: opacity 0.01s linear 0.13s;
  }
  img {
    position: absolute;
    inset: 0;
    border-radius: 22%;
    object-fit: contain;
    opacity: 0;
    transition: opacity 0.12s ease;
  }
  img.on {
    opacity: 1;
  }
  /* a light plate behind dark logos; normal icons are left as they are */
  .plate img {
    box-sizing: border-box;
    padding: 9%;
    background: #ececf0;
  }
  /* one moving highlight per icon, moved by the compositor only (no layout or paint work) */
  .shim {
    position: absolute;
    inset: 0;
    border-radius: 22%;
    overflow: hidden;
    background: #232323;
  }
  .shim::after {
    content: "";
    position: absolute;
    inset: 0;
    background: linear-gradient(100deg, transparent 20%, rgba(255, 255, 255, 0.07) 50%, transparent 80%);
    transform: translateX(-100%);
    animation: shim 1.3s ease-in-out infinite;
    will-change: transform;
  }
  @keyframes shim {
    to {
      transform: translateX(100%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .shim::after {
      animation: none;
      opacity: 0;
    }
  }
  .code {
    position: relative;
    font-weight: 600;
    letter-spacing: -0.02em;
    transition: opacity 0.12s ease;
  }
  .code.gone {
    opacity: 0;
  }
</style>
