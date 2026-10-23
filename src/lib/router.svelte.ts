// Tiny hash router: #/home, #/installed, #/installed/<appId>, #/get, #/get/<catalogId>, #/queue, #/activity, #/drivers, #/services, #/settings/<section>
export type RouteName = "home" | "installed" | "detail" | "get" | "getdetail" | "queue" | "activity" | "drivers" | "services" | "settings";
export const SETTINGS_SECTIONS = ["general", "apps", "downloads", "startup", "cleanup", "adobe", "about"] as const;
export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

export interface Route {
  name: RouteName;
  param: string | null;
}

function parse(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  const [a, b] = parts;
  switch (a) {
    case "installed":
      return b ? { name: "detail", param: b } : { name: "installed", param: null };
    case "get":
      return b ? { name: "getdetail", param: b } : { name: "get", param: null };
    case "queue":
    case "activity":
    case "drivers":
    case "services":
      return { name: a, param: null };
    case "settings":
      return { name: "settings", param: b ?? null };
    default:
      return { name: "home", param: null };
  }
}

interface HistState {
  idx: number;
}

class Router {
  route = $state<Route>(parse(typeof location !== "undefined" ? location.hash : ""));
  /** Bumped by every navigation (not by replace), so the page host can tell a new visit from a re-render. */
  navId = $state(0);
  /** scrollTop to restore after the page for the current entry renders; null for a new visit (starts at the top) */
  pendingScroll: number | null = null;
  /** The page host sets this; returns the scrollTop of the page that is on screen, or null while a transition is running. */
  scrollSource: (() => number | null) | null = null;
  /** The app sets this; the mouse side buttons do nothing while it returns false (a dialog is open). */
  canStep: (() => boolean) | null = null;

  /** Position in this window's history; stored in history.state so back and forward can find the entry again. */
  private cur = 0;
  private curHash = "";
  /** scroll position and hash per history entry */
  private scrolls = new Map<number, { hash: string; top: number }>();
  private hashes = new Map<number, string>();
  private lastTraversal = 0;

  constructor() {
    if (typeof window === "undefined") return;
    try {
      history.scrollRestoration = "manual";
    } catch {
      /* ignore */
    }
    const st = history.state as HistState | null;
    this.cur = typeof st?.idx === "number" ? st.idx : 0;
    if (typeof st?.idx !== "number") history.replaceState({ idx: this.cur } satisfies HistState, "", location.href);
    this.curHash = location.hash;
    this.hashes.set(this.cur, this.curHash);
    // back, forward and plain links (a href="#/...") all end up here
    window.addEventListener("popstate", () => this.onNav(true));
    window.addEventListener("hashchange", () => this.onNav(false));
    // mouse buttons 4 and 5 (back and forward on most mice)
    const side = (e: MouseEvent) => {
      if (e.button !== 3 && e.button !== 4) return;
      e.preventDefault();
      if (e.type !== "mouseup" || this.canStep?.() === false) return;
      // a webview that already went back by itself must not go back twice
      if (performance.now() - this.lastTraversal < 120) return;
      if (e.button === 3) this.back();
      else this.forward();
    };
    window.addEventListener("mousedown", side, true);
    window.addEventListener("mouseup", side, true);
    window.addEventListener("auxclick", (e) => (e.button === 3 || e.button === 4) && e.preventDefault(), true);
  }

  private saveCurrent() {
    const top = this.scrollSource?.();
    if (top != null) this.scrolls.set(this.cur, { hash: this.curHash, top });
  }
  private dropFrom(idx: number) {
    for (const k of [...this.scrolls.keys()]) if (k >= idx) this.scrolls.delete(k);
    for (const k of [...this.hashes.keys()]) if (k >= idx) this.hashes.delete(k);
  }

  private onNav(traversal: boolean) {
    const st = history.state as HistState | null;
    let idx = typeof st?.idx === "number" ? st.idx : null;
    // the second event of the same navigation (popstate, then hashchange)
    if (idx === this.cur && location.hash === this.curHash) return;
    if (traversal) this.lastTraversal = performance.now();
    this.saveCurrent();
    if (idx === null) {
      // a new visit made by a plain link: give the entry a number
      idx = this.cur + 1;
      history.replaceState({ idx } satisfies HistState, "", location.href);
      this.dropFrom(idx);
      this.pendingScroll = null;
    } else {
      const s = this.scrolls.get(idx);
      this.pendingScroll = s && s.hash === location.hash ? s.top : 0;
    }
    this.cur = idx;
    this.curHash = location.hash;
    this.hashes.set(idx, this.curHash);
    this.navId++;
    this.route = parse(location.hash);
  }

  go(path: string) {
    const target = path.startsWith("#") ? path : "#/" + path.replace(/^\//, "");
    if (location.hash === target) return;
    this.saveCurrent();
    const idx = this.cur + 1;
    this.dropFrom(idx);
    history.pushState({ idx } satisfies HistState, "", target);
    this.cur = idx;
    this.curHash = target;
    this.hashes.set(idx, target);
    this.pendingScroll = null;
    this.navId++;
    this.route = parse(target);
  }
  /** Section change inside settings without a history entry */
  replace(path: string) {
    const target = "#/" + path.replace(/^\//, "");
    history.replaceState({ idx: this.cur } satisfies HistState, "", target);
    this.curHash = target;
    this.hashes.set(this.cur, target);
    this.route = parse(target);
  }
  back() {
    history.back();
  }
  forward() {
    history.forward();
  }
  /**
   * A "back to X" link: when the entry before this one is X, step back so the scroll position of X comes back;
   * otherwise it is a normal visit.
   */
  up(path: string) {
    const target = path.startsWith("#") ? path : "#/" + path.replace(/^\//, "");
    if (this.hashes.get(this.cur - 1) === target) history.back();
    else this.go(target);
  }
  /** key for transitions: detail pages for different apps count as different pages */
  get key() {
    const r = this.route;
    return r.name === "detail" || r.name === "getdetail" ? `${r.name}:${r.param}` : r.name;
  }
}

export const router = new Router();

export function appHref(id: string) {
  return "#/installed/" + encodeURIComponent(id);
}

export function getHref(id: string) {
  return "#/get/" + encodeURIComponent(id);
}
