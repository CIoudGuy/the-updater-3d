<script lang="ts">
  import { tick } from "svelte";
  import Paint from "./Paint.svelte";
  import Icon from "./Icon.svelte";
  import { I } from "../lib/icons";
  import { ui } from "../lib/ui.svelte";
  import { pop } from "../lib/motion";

  const p = $derived(ui.popover);
  let root = $state<HTMLElement>();
  let marker = $state<HTMLElement>();
  let els: Array<HTMLElement | undefined> = [];
  let hi = $state(-1);
  let animMarker = $state(false);

  const isMenu = $derived(p?.kind === "menu");
  const itemH = $derived(isMenu ? 32 : 30);
  const width = $derived(isMenu ? 256 : (p?.width ?? 180));
  const fullHeight = $derived(p ? 8 + p.items.reduce((n, it) => n + (it.divider ? 9 : itemH), 0) : 0);
  const height = $derived(Math.min(fullHeight, 336));

  const pos = $derived.by(() => {
    if (!p) return { left: 0, top: 0, origin: "top left" };
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left = p.align === "end" ? p.rect.right - width : p.rect.left;
    left = Math.max(8, Math.min(left, vw - width - 8));
    let top = p.rect.bottom + 4;
    let v = "top";
    if (top + height > vh - 8) {
      top = Math.max(8, p.rect.top - 4 - height);
      v = "bottom";
    }
    top = Math.max(8, Math.min(top, vh - height - 8));
    return { left, top, origin: `${v} ${p.align === "end" ? "right" : "left"}` };
  });

  function enabled(i: number) {
    const it = p?.items[i];
    return !!it && !it.divider && !it.disabled;
  }
  function move(dir: 1 | -1) {
    if (!p) return;
    const n = p.items.length;
    let i = hi;
    for (let k = 0; k < n; k++) {
      i = (i + dir + n) % n;
      if (enabled(i)) {
        hi = i;
        return;
      }
    }
  }
  function pick(i: number) {
    if (!p || !enabled(i)) return;
    const id = p.items[i].id;
    const fn = p.onpick;
    ui.closePopover();
    fn(id);
  }

  function layoutMarker() {
    const el = els[hi];
    if (!marker || !el || hi < 0) {
      if (marker) marker.style.opacity = "0";
      return;
    }
    marker.style.opacity = "1";
    marker.style.height = el.offsetHeight + "px";
    marker.style.transform = `translateY(${el.offsetTop}px)`;
    el.scrollIntoView({ block: "nearest" });
  }

  $effect(() => {
    // open: choose the starting item and focus the layer
    const cur = p;
    if (!cur) {
      animMarker = false;
      return;
    }
    const sel = cur.selected ? cur.items.findIndex((x) => x.id === cur.selected) : -1;
    hi = sel >= 0 ? sel : cur.items.findIndex((_, i) => enabled(i));
    tick().then(() => {
      layoutMarker();
      root?.focus({ preventScroll: true });
      requestAnimationFrame(() => (animMarker = true));
    });
  });
  $effect(() => {
    hi;
    tick().then(layoutMarker);
  });

  function onkey(e: KeyboardEvent) {
    if (!p) return;
    const k = e.key;
    if (k === "ArrowDown") {
      e.preventDefault();
      move(1);
    } else if (k === "ArrowUp") {
      e.preventDefault();
      move(-1);
    } else if (k === "Home") {
      e.preventDefault();
      hi = p.items.findIndex((_, i) => enabled(i));
    } else if (k === "End") {
      e.preventDefault();
      hi = p.items.map((_, i) => i).filter(enabled).pop() ?? hi;
    } else if (k === "Enter" || k === " ") {
      e.preventDefault();
      pick(hi);
    } else if (k === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      ui.closePopover();
    } else if (k === "Tab") {
      e.preventDefault();
      ui.closePopover();
    } else if (isMenu && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const want = k === "Delete" ? "del" : k.toLowerCase();
      const i = p.items.findIndex((it) => it.key?.toLowerCase() === want && !it.disabled);
      if (i >= 0) {
        e.preventDefault();
        pick(i);
      }
    }
  }
</script>

{#if p}
  <div class="backdrop" role="presentation" onpointerdown={() => ui.closePopover()} oncontextmenu={(e) => e.preventDefault()}></div>
  <div
    class="pop"
    class:menu={isMenu}
    bind:this={root}
    tabindex="-1"
    role={isMenu ? "menu" : "listbox"}
    aria-label={p.label}
    style="left:{pos.left}px;top:{pos.top}px;width:{width}px"
    onkeydown={onkey}
    in:pop|global={{ origin: pos.origin }}
    out:pop|global={{ origin: pos.origin, duration: 70 }}
  >
    <Paint kind="blob" fill="#262626" />
    <div class="scroll" style="max-height:{height - 8}px">
    <span class="hi" class:anim={animMarker} bind:this={marker}><Paint kind="swatch" fill="#323232" /></span>
    <div class="items">
      {#each p.items as it, i (it.id + i)}
        {#if it.divider}
          <div class="div"></div>
        {:else if it.skeleton}
          <div class="sk" style="height:{itemH}px" aria-hidden="true"><span style="width:{48 + ((i * 17) % 34)}%"></span></div>
        {:else}
          <button
            bind:this={els[i]}
            class="item"
            class:danger={it.danger}
            role={isMenu ? "menuitem" : "option"}
            aria-selected={!isMenu ? p.selected === it.id : undefined}
            disabled={it.disabled}
            style="height:{itemH}px"
            tabindex="-1"
            onpointerenter={() => enabled(i) && (hi = i)}
            onclick={() => pick(i)}
          >
            {#if it.icon && isMenu}<Icon d={I[it.icon as keyof typeof I]} class="ic" />{/if}
            <span class="lbl">{it.label}</span>
            {#if it.meta}<span class="key">{it.meta}</span>{/if}
            {#if isMenu && it.key}<span class="key">{it.key}</span>{/if}
            {#if !isMenu && p.selected === it.id}<Icon d={I.check} size={14} width={2.5} />{/if}
          </button>
        {/if}
      {/each}
    </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .pop {
    position: fixed;
    z-index: 61;
    padding: 4px;
  }
  .scroll {
    position: relative;
    overflow-y: auto;
    overflow-x: hidden;
    scrollbar-width: thin;
  }
  .pop:focus-visible {
    outline: none;
  }
  .items {
    position: relative;
    display: flex;
    flex-direction: column;
  }
  .hi {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    opacity: 0;
    pointer-events: none;
  }
  .hi.anim {
    transition: transform 0.07s var(--ease), height 0.07s, opacity 0.08s;
  }
  .item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 10px;
    font-size: 13px;
    color: var(--text);
    text-align: left;
    width: 100%;
  }
  .item :global(.ic) {
    color: var(--text-2);
  }
  .item.danger,
  .item.danger :global(.ic) {
    color: var(--err);
  }
  .item:disabled {
    color: var(--text-4);
  }
  .lbl {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .key {
    font-size: 12px;
    color: var(--text-3);
  }
  .sk {
    display: flex;
    align-items: center;
    padding: 0 10px;
  }
  .sk span {
    height: 10px;
    border-radius: 4px;
    background: #323232;
    animation: shimmer 1.2s ease-in-out infinite;
  }
  .div {
    height: 1px;
    margin: 4px 8px;
    background: var(--sep-raised);
  }
  .pop:not(.menu) .item {
    justify-content: space-between;
  }
  .pop:not(.menu) .item .key {
    white-space: nowrap;
  }
  .pop:not(.menu) .item :global(svg) {
    position: relative;
  }
</style>
