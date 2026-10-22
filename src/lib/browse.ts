// Browse all (Get apps, Other apps): the whole winget catalog, a page at a time.
import type { CatalogEntry, OtherDetail } from "./types";

/** Entries loaded per page while scrolling. */
export const BROWSE_PAGE = 60;

/** First letters to browse by; "#" is everything that does not start with A to Z. */
export const LETTERS = ["#", ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ"];

export interface BrowseState {
  category: string | null;
  letter: string | null;
  entries: CatalogEntry[];
  total: number;
  categories: { name: string; count: number }[];
}

/** "15,724 apps from Microsoft's winget catalog." */
export function catalogNote(packages: number): string {
  return `${packages.toLocaleString("en-US")} ${packages === 1 ? "app" : "apps"} from Microsoft's winget catalog.`;
}

/** Cards that got their description or verdict: a new list when anything changed, else null. */
export function patchEntries(list: CatalogEntry[], patches: OtherDetail[]): CatalogEntry[] | null {
  const by = new Map(patches.map((p) => [p.id, p]));
  let changed = false;
  const out = list.map((e) => {
    const p = by.get(e.id);
    if (!p) return e;
    const next = { ...e, tagline: p.tagline ?? e.tagline, description: e.description ?? p.description, blocked: p.blocked };
    if (next.tagline === e.tagline && next.description === e.description && next.blocked === e.blocked) return e;
    changed = true;
    return next;
  });
  return changed ? out : null;
}

/** Add a loaded page after the ones shown: entries already there are not repeated. */
export function appendPage(have: CatalogEntry[], page: CatalogEntry[]): CatalogEntry[] {
  const ids = new Set(have.map((e) => e.id));
  return [...have, ...page.filter((e) => !ids.has(e.id))];
}

/** The short reason shown next to a disabled Install. */
export function blockedTitle(reason: string): string {
  return `Can't be installed silently: ${reason}`;
}
