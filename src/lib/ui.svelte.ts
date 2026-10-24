// Transient UI state: popovers, dialogs, view preferences.
import type { AppEntry, CatalogEntry } from "./types";
import type { BrowseState, OtherSort } from "./browse";
import { focusAfterLeave } from "./keys";
import { store } from "./store.svelte";
import { listedOnly } from "./view";

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
  /** a search box above a long list, with this placeholder */
  search?: string;
}

export type DialogState =
  | { type: "uninstall"; appId: string }
  | { type: "uninstall-many"; appIds: string[] }
  /** after uninstalling: waits for the uninstalls, then shows what they left behind */
  | { type: "leftovers"; apps: { id: string; name: string; broken?: boolean }[] }
  | {
      type: "lowdisk";
      name: string;
      /** "Before installing X" */
      eyebrow: string;
      /** "needs" or "need" */
      needs: string;
      drive: string;
      needed: number;
      free: number;
      cacheSize: number;
      relocate: { drive: string; free: number } | null;
      proceed: () => void;
      onClearCache: () => void;
      onRelocate: () => void;
    };

export type InstalledFilter = "updates" | "all" | "games" | "drivers" | "recent";
export type InstalledGroup = "all" | "Adobe" | "Craft apps" | "Blender" | "More apps" | "Other apps" | "Games" | "Drivers" | "Other programs";
/** Installed: every program, or only the apps a catalog updates. */
export type InstalledScope = "everything" | "catalog";
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
  /** "Update and restart" was clicked: downloading the new version, the app restarts by itself */
  selfUpdating = $state(false);

  view = $state<"grid" | "list">(load("view", ["grid", "list"] as const, "grid"));
  filter = $state<InstalledFilter>("all");
  group = $state<InstalledGroup>("all");
  scope = $state<InstalledScope>(load("scope", ["everything", "catalog"] as const, "everything"));
  /** Installed: picking apps for a batch uninstall, and the ones picked */
  selecting = $state(false);
  selected = $state<string[]>([]);
  sort = $state<SortKey>(load("sort", ["name", "size", "updated", "status"] as const, "name"));
  query = $state("");
  getQuery = $state("");
  /** Get apps: the source shown (a search looks through all of them) */
  getGroup = $state<"Adobe" | "Craft apps" | "Blender" | "More apps" | "Other apps">("Other apps");

  /** Get apps list kept across visits, so coming back from a detail page does not reload it */
  catalog = $state<CatalogEntry[] | null>(null);
  /** Other apps: the pages of the whole winget catalog loaded so far (or of a search through
   * it), kept across visits like the catalog */
  otherBrowse = $state.raw<BrowseState | null>(null);
  /** Other apps order, category and first letter, kept for this session */
  otherSort = $state<OtherSort>("popular");
  otherCategory = $state<string | null>(null);
  otherLetter = $state<string | null>(null);
  /** Packages in the whole winget catalog, for the note under Other apps */
  otherPackages = $state<number | null>(null);

  searchEl: HTMLInputElement | null = null;
  /** app under the pointer or keyboard focus, target of the U shortcut */
  focusedApp = $state<string | null>(null);

  compact = $derived(this.width < 1000);

  /** the pointer or focus left this app (or the screen changed) */
  leaveApp(id: string) {
    this.focusedApp = focusAfterLeave(this.focusedApp, id);
  }
  setView(v: "grid" | "list") {
    this.view = v;
    save("view", v);
  }
  setScope(s: InstalledScope) {
    this.scope = s;
    save("scope", s);
  }
  toggleSelected(id: string) {
    this.selected = this.selected.includes(id) ? this.selected.filter((x) => x !== id) : [...this.selected, id];
  }
  endSelecting() {
    this.selecting = false;
    this.selected = [];
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

/** The app's updates are turned off ("Don't update"). */
export function isIgnored(id: string): boolean {
  return (store.settings.ignored ?? []).includes(id);
}

export function menuItemsFor(app: AppEntry): PopItem[] {
  const items: PopItem[] = [];
  const skip: PopItem = isIgnored(app.id) ? { id: "unignore", label: "Show its updates again", icon: "play" } : { id: "ignore", label: "Don't update", icon: "pause" };
  if (app.source === "games") {
    // Updated and uninstalled in its launcher.
    items.push({ id: "notes", label: "Patch notes", icon: "notes", key: "N" });
    if (app.installDir) items.push({ id: "folder", label: "Show in folder", icon: "folder", key: "F" });
    items.push(skip);
    items.push({ id: "sep", label: "", divider: true });
    items.push({ id: "uninstall", label: "Uninstall", icon: "trash", key: "Del", danger: true, disabled: !app.canUninstall });
    return items;
  }
  if (app.source === "drivers") {
    items.push({ id: "notes", label: "Release notes", icon: "notes", key: "N" });
    if (app.manualUrl) items.push({ id: "maker", label: "Open the maker's page", icon: "external" });
    items.push(skip);
    return items;
  }
  if (listedOnly(app)) {
    // No catalog: no notes or other versions. A broken one is removed from the list instead.
    if (app.installDir && app.status !== "missing") items.push({ id: "folder", label: "Show in folder", icon: "folder", key: "F" });
    if (items.length) items.push({ id: "sep", label: "", divider: true });
    items.push(app.status === "missing" ? { id: "uninstall", label: "Remove from list", icon: "trash", key: "Del", danger: true } : { id: "uninstall", label: "Uninstall", icon: "trash", key: "Del", danger: true, disabled: !app.canUninstall });
    return items;
  }
  const repairable = canRepair(app);
  if (app.status === "missing" && repairable) items.push({ id: "repair", label: "Repair", icon: "repair", key: "R" });
  items.push({ id: "notes", label: "Release notes", icon: "notes", key: "N" });
  if (!app.readOnly) items.push({ id: "version", label: "Install another version", icon: "version", key: "V" });
  if (app.storeUrl) items.push({ id: "store", label: "Open in Microsoft Store", icon: "external", key: "S" });
  if (repairable && app.status !== "missing") items.push({ id: "repair", label: "Repair", icon: "repair", key: "R" });
  if (app.status !== "missing") items.push({ id: "folder", label: "Show in folder", icon: "folder", key: "F" });
  if (!app.readOnly) items.push(skip);
  items.push({ id: "sep", label: "", divider: true });
  items.push({ id: "uninstall", label: "Uninstall", icon: "trash", key: "Del", danger: true, disabled: !app.canUninstall });
  return items;
}

/** Repair is valid for Adobe and Craft apps; the backend rejects Blender. */
export function canRepair(app: AppEntry): boolean {
  return app.source === "adobe" || app.source === "craft";
}
