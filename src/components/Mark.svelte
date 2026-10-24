<script lang="ts" module>
  // Pictures that were in the page once come back without a fade or a shimmer when a card scrolls
  // away and returns, and a dark logo is looked at once.
  const seen = new Set<string>();
  const darkOf = new Map<string, boolean>();
</script>

<script lang="ts">
  import { MARK, MARK_LINES, MARK_LINES_BIG } from "../lib/paths";
  import { mostlyDark } from "../lib/iconlook";
  import { isPainted, markShow, type Load } from "../lib/markstate";

  // An app shows its icon when it has one. Until then, and when it has none or the picture cannot
  // be loaded, it shows a default "no image" tile drawn here (never an <img> that could show the
  // browser's broken-image glyph); a soft shimmer runs over the tile while an icon is on its way.
  // Apps with a painted brand mark (Adobe, Blender, ...) keep that mark instead of the tile.
  let {
    app,
    size = 32,
    fs = 0,
    dim = false,
    eager = true,
  }: {
    app: { id?: string; code: string; fg: string; bg: string; iconUrl: string | null; iconMiss?: boolean };
    size?: number;
    fs?: number;
    dim?: boolean;
    /** icons in the first screenful decode right away; pass false for long lists below the fold */
    eager?: boolean;
  } = $props();

  let loadedUrl = $state<string | null>(null);
  let failedUrl = $state<string | null>(null);
  const url = $derived(app.iconUrl);
  const load = $derived<Load>(!url ? "none" : failedUrl === url ? "failed" : loadedUrl === url ? "loaded" : "loading");
  const show = $derived(markShow(app, load));
  const painted = $derived(isPainted(app.id));
  const loaded = $derived(load === "loaded");
  const font = $derived(fs || Math.max(9, Math.round(size * 0.4)));
  /** a picture met before: no fade, so a card that scrolls back does not flicker */
  const known = $derived(!!url && seen.has(url));

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
    if (!url) return;
    seen.add(url);
    if (/\.(dark|icon)\.png($|[?#])/i.test(url)) return;
    let dark = darkOf.get(url);
    if (dark === undefined) {
      dark = looksDark(img);
      darkOf.set(url, dark);
    }
    if (dark) darkUrl = url;
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
  {#if painted}
    <svg class:gone={loaded} viewBox="0 0 48 48" width={size} height={size}>
      <path d={MARK} fill={app.bg} />
      <path d={size >= 64 ? MARK_LINES_BIG : MARK_LINES} fill="none" stroke={app.fg} stroke-opacity="0.3" stroke-width="1.6" stroke-linecap="round" />
    </svg>
    <span class="code" class:gone={loaded} style="font-size:{font}px;color:{app.fg}">{app.code}</span>
  {:else}
    <!-- the default "no image" tile: a soft rounded tile with a plain picture glyph -->
    <svg class="ph" class:gone={loaded} viewBox="0 0 48 48" width={size} height={size}>
      <rect width="48" height="48" rx="11" fill="var(--ph-tile)" />
      <g fill="none" stroke="var(--ph-glyph)" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="12.5" y="13.5" width="23" height="21" rx="4.5" />
        <circle cx="19.5" cy="21" r="2.2" />
        <path d="M13.5 31.5l7-6.5 5 4.5 3.5-3 6 5" />
      </g>
    </svg>
    {#if show.shimmer}<span class="shim"></span>{/if}
  {/if}
  {#if show.image}
    {#key url}
      <img
        class:on={loaded}
        class:known
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
    --ph-tile: var(--raised, #262626);
    --ph-glyph: var(--text-4, #6e6e6e);
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
  /* once the real icon is fully in, drop the mark or tile underneath so transparent corners stay clean */
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
  img.known {
    transition: none;
  }
  /* a light plate behind dark logos; normal icons are left as they are */
  .plate img {
    box-sizing: border-box;
    padding: 9%;
    background: #ececf0;
  }
  /* one moving highlight per icon on its way, moved by the compositor only (no layout or paint work) */
  .shim {
    position: absolute;
    inset: 0;
    border-radius: 22%;
    overflow: hidden;
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
