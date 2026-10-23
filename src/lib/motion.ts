// Motion helpers: transform, opacity and clip-path only.
import { cubicOut } from "svelte/easing";

export const EASE = "cubic-bezier(0.2, 0.8, 0.2, 1)";

let query: MediaQueryList | null = null;
let lean = false;

/** Lighter animations on a low end PC (as with Windows' "reduce animations"). */
export function setLean(on: boolean) {
  lean = on;
}

export function reduced(): boolean {
  if (lean) return true;
  if (typeof matchMedia === "undefined") return false;
  // One media query, read again on every call (rows and tiles ask once each).
  query ??= matchMedia("(prefers-reduced-motion: reduce)");
  return query.matches;
}

/** Stagger-in for tiles and rows: use:enter={index} */
export function enter(node: HTMLElement, index: number | { i: number; y?: number }) {
  const i = typeof index === "number" ? index : index.i;
  const y = typeof index === "number" ? 8 : (index.y ?? 8);
  if (reduced()) {
    node.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 80, fill: "backwards" });
    return;
  }
  node.animate(
    [
      { opacity: 0, transform: `translateY(${y}px)` },
      { opacity: 1, transform: "translateY(0)" },
    ],
    { duration: 170, delay: Math.min(i, 10) * 14, easing: EASE, fill: "backwards" },
  );
}

/** Svelte transition: scale from 0.96 and fade, from a given origin */
export function pop(_node: Element, { duration = 95, origin = "top right", from = 0.96 }: { duration?: number; origin?: string; from?: number } = {}) {
  return {
    duration: reduced() ? 0 : duration,
    easing: cubicOut,
    css: (t: number) => `opacity:${t};transform:scale(${from + (1 - from) * t});transform-origin:${origin}`,
  };
}

/** Slide in from the right (toasts) */
export function slideIn(_node: Element, { duration = 150, x = 28 }: { duration?: number; x?: number } = {}) {
  return {
    duration: reduced() ? 0 : duration,
    easing: cubicOut,
    css: (t: number) => `opacity:${t};transform:translateX(${(1 - t) * x}px)`,
  };
}

/** Quick fade and rise for items that appear later */
export function rise(_node: Element, { duration = 130, y = 6 }: { duration?: number; y?: number } = {}) {
  return {
    duration: reduced() ? 0 : duration,
    easing: cubicOut,
    css: (t: number) => `opacity:${t};transform:translateY(${(1 - t) * y}px)`,
  };
}

/** Settings pages: the new page slides in a little from the side it comes from, the old one fades out quickly. */
export function pageIn(_node: Element, { dir = 1, duration = 130, x = 16 }: { dir?: number; duration?: number; x?: number } = {}) {
  return {
    delay: reduced() ? 0 : 30,
    duration: reduced() ? 0 : duration,
    easing: cubicOut,
    css: (t: number) => `opacity:${t};transform:translateX(${(1 - t) * x * dir}px)`,
  };
}
export function pageOut(_node: Element, { dir = 1, duration = 70, x = 10 }: { dir?: number; duration?: number; x?: number } = {}) {
  return {
    duration: reduced() ? 0 : duration,
    easing: cubicOut,
    css: (t: number) => `opacity:${t};transform:translateX(${(1 - t) * -x * dir}px);pointer-events:none`,
  };
}

export const MARKER_BASE = 100;
/**
 * Place a sliding marker over an element inside the same positioned parent.
 * The marker is absolute, left 0, top 0, 100px wide; it moves with translateX and stretches with scaleX.
 */
export function place(marker: HTMLElement, el: HTMLElement | null | undefined, inset = { x: 0, top: 0, bottom: 0 }) {
  if (!el) {
    marker.style.opacity = "0";
    return;
  }
  const x = el.offsetLeft + inset.x;
  const w = Math.max(0, el.offsetWidth - inset.x * 2);
  marker.style.opacity = "1";
  marker.style.height = `${el.offsetHeight - inset.top - inset.bottom}px`;
  marker.style.top = `${el.offsetTop + inset.top}px`;
  marker.style.transform = `translateX(${x}px) scaleX(${w / MARKER_BASE})`;
}
