// Page zoom of the main window: Ctrl +, Ctrl -, Ctrl 0 and Ctrl + mouse wheel.
// Inside Tauri it uses the webview's own zoom. In a plain browser (the mock preview) it falls back to CSS zoom on the root.
import { isTauri } from "./api";

export const ZOOM_STEPS = [0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2] as const;
const KEY = "uiZoom";

let current = 1;
let cssFallback = !isTauri;

export const zoomPercent = (z: number) => `${Math.round(z * 100)}%`;
export const currentZoom = () => current;
/** How much CSS zoom scales the layout (1 when the webview itself zooms). */
export const cssScale = () => (cssFallback ? current : 1);

/** Index of the step closest to a zoom factor. */
function nearest(z: number): number {
  let best = 0;
  for (let i = 1; i < ZOOM_STEPS.length; i++) if (Math.abs(ZOOM_STEPS[i] - z) < Math.abs(ZOOM_STEPS[best] - z)) best = i;
  return best;
}
export function clampZoom(z: number): number {
  return Number.isFinite(z) ? ZOOM_STEPS[nearest(z)] : 1;
}
/** The step above or below the current zoom. */
export function stepZoom(dir: 1 | -1 | 0): number {
  if (dir === 0) return 1;
  const i = nearest(current);
  return ZOOM_STEPS[Math.max(0, Math.min(ZOOM_STEPS.length - 1, i + dir))];
}

function applyCss(z: number) {
  const root = document.documentElement;
  // CSS zoom also scales vh and vw, so the app shell is sized from the unzoomed viewport
  root.style.setProperty("--zoom", String(z));
  root.style.zoom = z === 1 ? "" : String(z);
}

/** Apply a zoom level to the window. Never throws. */
export async function applyZoom(z: number): Promise<void> {
  z = clampZoom(z);
  current = z;
  try {
    localStorage.setItem(KEY, String(z));
  } catch {
    /* storage can be unavailable */
  }
  if (!cssFallback) {
    try {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      await getCurrentWebview().setZoom(z);
      return;
    } catch {
      // the webview refused (permission missing or API unavailable): use CSS zoom instead
      cssFallback = true;
    }
  }
  applyCss(z);
}

/** Zoom used at the last run, read synchronously so the first paint already has it. */
export function cachedZoom(): number {
  try {
    const v = Number(localStorage.getItem(KEY));
    if (v > 0) return clampZoom(v);
  } catch {
    /* ignore */
  }
  return 1;
}
