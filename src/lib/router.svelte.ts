// Tiny hash router: #/home, #/installed, #/installed/<appId>, #/get, #/queue, #/activity, #/services, #/settings/<section>
export type RouteName = "home" | "installed" | "detail" | "get" | "queue" | "activity" | "services" | "settings";
export const SETTINGS_SECTIONS = ["sources", "updates", "notifications", "background", "downloads", "storage", "account", "about"] as const;
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
    case "queue":
    case "activity":
    case "services":
      return { name: a, param: null };
    case "settings":
      return { name: "settings", param: b ?? null };
    default:
      return { name: "home", param: null };
  }
}

class Router {
  route = $state<Route>(parse(typeof location !== "undefined" ? location.hash : ""));

  constructor() {
    if (typeof window !== "undefined") {
      window.addEventListener("hashchange", () => (this.route = parse(location.hash)));
    }
  }
  go(path: string) {
    const target = path.startsWith("#") ? path : "#/" + path.replace(/^\//, "");
    if (location.hash === target) return;
    location.hash = target;
    this.route = parse(target);
  }
  /** Section change inside settings without a history entry */
  replace(path: string) {
    const target = "#/" + path.replace(/^\//, "");
    history.replaceState(null, "", target);
    this.route = parse(target);
  }
  back() {
    history.back();
  }
  /** key for transitions: detail pages for different apps count as different pages */
  get key() {
    const r = this.route;
    return r.name === "detail" ? `detail:${r.param}` : r.name;
  }
}

export const router = new Router();

export function appHref(id: string) {
  return "#/installed/" + encodeURIComponent(id);
}
