// Transient UI state: popovers, dialogs, view preferences.
import type { AppEntry } from "./types";

export interface PopItem {
  id: string;
  label: string;
  icon?: string;
  key?: string;
  danger?: boolean;
  disabled?: boolean;
  divider?: boolean;
  /** right-aligned secondary text, for example "Latest, 390 MB" */
  meta?: string;
  /** placeholder row while a list loads */
  skeleton?: boolean;
}
export interface Popover {
  kind: "menu" | "list";
  rect: DOMRect;
  items: PopItem[];
  selected?: string;
  width?: number;
  align: "start" | "end";
  label: string;
  anchor: HTMLElement | null;
  onpick: (id: string) => void;
}

export type DialogState =
  | { type: "uninstall"; appId: string }
  | {
      type: "lowdisk";
      name: string;
      drive: string;
      needed: number;
      free: number;
      cacheSize: number;
      relocate: { drive: string; free: number } | null;
      proceed: () => void;
      onClearCache: () => void;
      onRelocate: () => void;
    };

export type InstalledFilter = "updates" | "all" | "recent";
export type InstalledGroup = "all" | "Adobe" | "Craft apps" | "Blender";
export type SortKey = "name" | "size" | "updated" | "status";

function load<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    if (v && (allowed as readonly string[]).includes(v)) return v as T;
  } catch {
    /* storage can be unavailable */
  }
  return fallback;
}
function save(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* ignore */
  }
}

class Ui {
  popover = $state<Popover | null>(null);
  dialog = $state<DialogState | null>(null);
  dismissedBlocked = $state<string[]>([]);
  width = $state(typeof window !== "undefined" ? window.innerWidth : 1120);
  maximized = $state(false);
  /** set by the "Install another version" menu item; the detail page then focuses the picker */
  pickVersion = $state(false);

  view = $state<"grid" | "list">(load("view", ["grid", "list"] as const, "grid"));
  filter = $state<InstalledFilter>("all");
  group = $state<InstalledGroup>("all");
  sort = $state<SortKey>(load("sort", ["name", "size", "updated", "status"] as const, "name"));
  query = $state("");
  getQuery = $state("");
  /** Get apps source filter: "all", or a group name */
  getGroup = $state<"all" | "Adobe" | "Craft apps" | "Blender">("all");

  searchEl: HTMLInputElement | null = null;
  /** app under the pointer or keyboard focus, target of the U shortcut */
  focusedApp = $state<string | null>(null);

  compact = $derived(this.width < 1000);

  setView(v: "grid" | "list") {
    this.view = v;
    save("view", v);
  }
  setSort(s: SortKey) {
    this.sort = s;
    save("sort", s);
  }
  openMenu(p: Omit<Popover, "kind">) {
    this.popover = { ...p, kind: "menu" };
  }
  openList(p: Omit<Popover, "kind">) {
    this.popover = { ...p, kind: "list" };
  }
  closePopover() {
    const a = this.popover?.anchor;
    this.popover = null;
    a?.focus?.({ preventScroll: true });
  }
  openDialog(d: DialogState) {
    this.popover = null;
    this.dialog = d;
  }
  closeDialog() {
    this.dialog = null;
  }
  /** Escape: closes the top layer, returns true if something closed */
  escape(): boolean {
    if (this.popover) {
      this.closePopover();
      return true;
    }
    if (this.dialog) {
      this.dialog = null;
      return true;
    }
    return false;
  }
  anyLayer() {
    return !!(this.popover || this.dialog);
  }
}

export const ui = new Ui();

export function menuItemsFor(app: AppEntry): PopItem[] {
  const items: PopItem[] = [];
  const repairable = canRepair(app);
  if (app.status === "missing" && repairable) items.push({ id: "repair", label: "Repair", icon: "repair", key: "R" });
  items.push({ id: "notes", label: "Release notes", icon: "notes", key: "N" });
  items.push({ id: "version", label: "Install another version", icon: "version", key: "V" });
  if (repairable && app.status !== "missing") items.push({ id: "repair", label: "Repair", icon: "repair", key: "R" });
  if (app.status !== "missing") items.push({ id: "folder", label: "Show in folder", icon: "folder", key: "F" });
  items.push({ id: "sep", label: "", divider: true });
  items.push({ id: "uninstall", label: "Uninstall", icon: "trash", key: "Del", danger: true, disabled: !app.canUninstall });
  return items;
}

/** Repair is valid for Adobe and Craft apps; the backend rejects Blender. */
export function canRepair(app: AppEntry): boolean {
  return app.source === "adobe" || app.source === "craft";
}
