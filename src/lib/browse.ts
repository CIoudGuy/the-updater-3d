// Other apps in Get apps: the whole winget catalog, a page at a time as the list is scrolled.
import type { CatalogEntry, OtherDetail, OtherIcon } from "./types";

/** Entries loaded per page while scrolling. */
export const BROWSE_PAGE = 60;

/** First letters to browse by; "#" is everything that does not start with A to Z. */
export const LETTERS = ["#", ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ"];

export type OtherSort = "popular" | "az" | "za" | "updated";

/** The orders of Other apps; with a search, "Popular first" lists the best matches first. */
export function sortOptions(searching: boolean): { value: OtherSort; label: string }[] {
  return [
    { value: "popular", label: searching ? "Best match" : "Popular first" },
    { value: "az", label: "Name A to Z" },
    { value: "za", label: "Name Z to A" },
    { value: "updated", label: "Recently updated" },
  ];
}

/** What a loaded list is for: a new key starts the list again. */
export interface BrowseKey {
  query: string;
  category: string | null;
  letter: string | null;
  sort: OtherSort;
}

/** A place in the list: its entry, or null while its page is not loaded yet. */
export type Slot = CatalogEntry | null;

export interface BrowseState extends BrowseKey {
  /** one place for every entry of the list, so the scrollbar spans all of it from the start */
  entries: Slot[];
  total: number;
  categories: { name: string; count: number }[];
  /** entries of the whole list with a known release date */
  dated: number;
}

export function sameKey(a: BrowseKey, b: BrowseKey): boolean {
  return a.query === b.query && a.category === b.category && a.letter === b.letter && a.sort === b.sort;
}

/** The honest note under "Recently updated": dates come from manifests read so far. */
export function datedNote(dated: number, total: number): string {
  if (total === 0) return "";
  if (dated >= total) return "Newest release first.";
  if (dated === 0) return "No release dates are known yet; they fill in as apps are shown. Listed A to Z for now.";
  return `Release dates are known for ${dated.toLocaleString("en-US")} of ${total.toLocaleString("en-US")} apps so far; the rest follow A to Z and get dates as they are shown.`;
}

const words = (s: string) => s.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
const compact = (s: string) => s.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");

/**
 * How well a curated entry (Adobe, Blender, ...) matches a search, best first, like the winget
 * catalog's ranking: 0 exact name, 1 name start, 2 every word starts a word of the name,
 * 3 inside the name, 4 every word starts a word of the line under it. null: no match.
 */
export function matchRank(name: string, line: string | null, query: string): number | null {
  const q = query.trim().toLowerCase();
  const qw = words(q);
  if (!qw.length) return null;
  const n = name.trim().toLowerCase();
  const qc = compact(q);
  const nc = compact(n);
  if (n === q || nc === qc) return 0;
  if (n.startsWith(q) || nc.startsWith(qc)) return 1;
  const nw = words(n);
  if (qw.every((w) => nw.some((x) => x.startsWith(w)))) return 2;
  if (n.includes(q) || nc.includes(qc)) return 3;
  const lw = words(line ?? "");
  if (qw.every((w) => nw.some((x) => x.startsWith(w)) || lw.some((x) => x.startsWith(w)))) return 4;
  return null;
}

/** Entries that match a search, best first (equal matches keep their order). */
export function rankEntries(list: CatalogEntry[], query: string): CatalogEntry[] {
  return list
    .map((e, i) => ({ e, i, r: matchRank(e.name, e.tagline, query) }))
    .filter((x) => x.r !== null)
    .sort((a, b) => (a.r as number) - (b.r as number) || a.i - b.i)
    .map((x) => x.e);
}

/** "15,724 apps from Microsoft's winget catalog." */
export function catalogNote(packages: number): string {
  return `${packages.toLocaleString("en-US")} ${packages === 1 ? "app" : "apps"} from Microsoft's winget catalog.`;
}

/** Cards that got their description or verdict: a new list when anything changed, else null. */
export function patchEntries<T extends Slot>(list: T[], patches: OtherDetail[]): T[] | null {
  const by = new Map(patches.map((p) => [p.id, p]));
  let changed = false;
  const out = list.map((e) => {
    const p = e && by.get(e.id);
    if (!e || !p) return e;
    const next = { ...e, tagline: p.tagline ?? e.tagline, description: e.description ?? p.description, blocked: p.blocked };
    if (next.tagline === e.tagline && next.description === e.description && next.blocked === e.blocked) return e;
    changed = true;
    return next as T;
  });
  return changed ? out : null;
}

/** Cards whose icon was found: a new list when any changed, else null. Nothing moves. */
export function patchIcons<T extends Slot>(list: T[], icons: OtherIcon[]): T[] | null {
  const by = new Map(icons.map((i) => [i.id, i.iconUrl]));
  let changed = false;
  const out = list.map((e) => {
    const u = e && by.get(e.id);
    if (!e || !u || e.iconUrl === u) return e;
    changed = true;
    return { ...e, iconUrl: u } as T;
  });
  return changed ? out : null;
}

/** Put a loaded page in its places. The list takes the length the backend gave (`total`); places
 * not loaded yet stay null. */
export function placePage(have: Slot[], total: number, offset: number, page: CatalogEntry[]): Slot[] {
  const out: Slot[] = Array.from({ length: total }, (_, i) => have[i] ?? null);
  page.forEach((e, i) => {
    if (offset + i < total) out[offset + i] = e;
  });
  return out;
}

/** Start offsets of the pages of `size` that have an unloaded place in [from, to), nearest to
 * `from` first, leaving out the ones in `skip` (asked for already). */
export function pagesToLoad(list: Slot[], from: number, to: number, size: number, skip: ReadonlySet<number> = new Set()): number[] {
  const out: number[] = [];
  const end = Math.min(to, list.length);
  for (let p = Math.floor(Math.max(0, from) / size) * size; p < end; p += size) {
    if (skip.has(p)) continue;
    for (let i = Math.max(p, from); i < Math.min(p + size, end); i++) {
      if (list[i] == null) {
        out.push(p);
        break;
      }
    }
  }
  return out;
}

/** Cards read again (icons found since): the list keeps its length and order. */
export function mergeEntries(have: Slot[], fresh: CatalogEntry[]): Slot[] {
  const by = new Map(fresh.map((e) => [e.id, e]));
  return have.map((e) => (e ? (by.get(e.id) ?? e) : e));
}

/**
 * The rows of a card grid to keep in the page: the ones in view and `overscan` pixels above and
 * below. `above` is how far the top of the grid is above the top of the view (negative while the
 * grid starts lower down). Every row is `rowH` tall, so nothing has to be measured.
 */
export function windowRows(above: number, viewH: number, rowH: number, rows: number, overscan: number): { first: number; last: number } {
  if (rows <= 0 || rowH <= 0) return { first: 0, last: 0 };
  const first = Math.min(rows, Math.max(0, Math.floor((above - overscan) / rowH)));
  const last = Math.min(rows, Math.max(first, Math.ceil((above + viewH + overscan) / rowH)));
  return { first, last };
}

/** The reason the backend gives for an app that only the Microsoft Store installs. */
export const STORE_ONLY = "only in the Microsoft Store";

/** Is this an app only the Microsoft Store installs (its page there is offered instead)? */
export function storeOnly(reason: string | null | undefined): boolean {
  return reason === STORE_ONLY;
}

/** The short reason shown next to a disabled Install. */
export function blockedTitle(reason: string): string {
  return storeOnly(reason) ? "Install from the Microsoft Store" : `Can't be installed silently: ${reason}`;
}
