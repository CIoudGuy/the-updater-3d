// Small pure helpers for what the screens say and show, so they can be tested without the UI.
import { countOf, nameList } from "./format.ts";
import type { Job, JobKind, Settings } from "./types";

/** Apps whose files are gone: the ones a reinstall brings back, and removed programs that only left their entry in Windows' list (nothing to repair: they are removed from the list). */
export function splitMissing<T extends { status: string; source: string }>(apps: readonly T[]): { repair: T[]; leftovers: T[] } {
  const gone = apps.filter((a) => a.status === "missing");
  return { repair: gone.filter((a) => a.source !== "programs"), leftovers: gone.filter((a) => a.source === "programs") };
}

/** An Other program nothing gives updates for (no catalog lists it, it names no GitHub project): listed to uninstall it. */
export function listedOnly(app: { source: string; github?: string | null }): boolean {
  return app.source === "programs" && !app.github;
}

/** The short description an app's page shows under its version, like Get apps does. An Other
 * program's line is its publisher and a driver's its kind (shown in Details), and a copy another
 * app manages already says so in its note. */
export function aboutLine(app: { source: string; tagline: string | null; note: string | null; readOnly?: boolean; github?: string | null }): string | null {
  if (listedOnly(app) || app.source === "drivers" || app.readOnly) return null;
  const t = app.tagline?.trim();
  return t && t !== app.note ? t : null;
}

/** The Home line about games whose launcher has an update waiting: what it says and its button. */
export function gameUpdatesLine(games: readonly { id: string; name: string }[]): { text: string; button: string } | null {
  if (!games.length) return null;
  if (games.length === 1) {
    const l = launcherOf(games[0].id);
    return { text: `${games[0].name} has an update waiting in ${l}.`, button: `Update in ${l}` };
  }
  return { text: `${games.length} games have updates waiting in their launchers.`, button: "Show them" };
}

/** The launcher a game came from, by name, from its id ("game:steam:1912410"). */
export function launcherOf(id: string): string {
  const k = id.startsWith("game:") ? id.split(":")[1] : "";
  return ({ steam: "Steam", epic: "Epic Games", ubisoft: "Ubisoft Connect", gog: "GOG Galaxy", ea: "EA app", battlenet: "Battle.net", amazon: "Amazon Games", itch: "itch", rockstar: "Rockstar" } as Record<string, string>)[k] ?? "its launcher";
}

/** The version in the Installed list's Latest column. An app whose updates are turned off shows the
 * version it has: its newer version shows only on its own page. */
export function latestColumn(app: { source: string; status: string; latest: string | null; version: string; github?: string | null }, updatesOff: boolean): string {
  if (listedOnly(app) || app.status === "missing") return "";
  if (app.status === "update") return app.latest ?? "";
  return updatesOff ? app.version : (app.latest ?? app.version);
}

/** A source filter that points at a source with no apps left falls back to "all". */
export function groupOrAll<T extends string>(group: T | "all", present: readonly T[]): T | "all" {
  return group !== "all" && present.includes(group as T) ? group : "all";
}

/** The Updates and Recently updated filters hide apps; say so when a search matches only hidden ones. */
export function installedNone(o: { query: string; filter: "updates" | "all" | "games" | "drivers" | "recent"; matchesIgnoringFilter: number }): { text: string; showAll: boolean } {
  const q = o.query.trim();
  if (q) {
    if (o.filter !== "all" && o.matchesIgnoringFilter > 0) {
      const f = o.filter === "updates" ? "Updates available" : o.filter === "drivers" ? "Drivers" : o.filter === "games" ? "Games" : "Recently updated";
      const n = o.matchesIgnoringFilter;
      return { text: `${n === 1 ? "1 app matches" : `${n} apps match`} "${q}", hidden by the ${f} filter.`, showAll: true };
    }
    return { text: `No apps match "${o.query}".`, showAll: false };
  }
  if (o.filter === "updates") return { text: "No updates available. Everything here is current.", showAll: true };
  if (o.filter === "drivers") return { text: "No drivers listed yet. They show once the next check is done.", showAll: true };
  if (o.filter === "games") return { text: "No games found. Games from Steam, Epic Games, Ubisoft Connect and other launchers show here.", showAll: true };
  return { text: "Nothing here yet.", showAll: false };
}

const NOUN: Record<JobKind, string> = { update: "update", install: "install", rollback: "rollback", repair: "repair", uninstall: "uninstall" };
const COULD_NOT: Record<JobKind, string> = {
  update: "could not update",
  install: "could not install",
  rollback: "could not roll back",
  repair: "could not be repaired",
  uninstall: "could not be uninstalled",
};

/** "1 update", "2 installs", or "3 jobs" when the failed jobs are of different kinds. */
export function failedCount(jobs: Pick<Job, "kind">[]): string {
  const kinds = new Set(jobs.map((j) => j.kind));
  if (kinds.size === 1) return countOf(jobs.length, NOUN[jobs[0].kind]);
  return countOf(jobs.length, "job");
}

/** "Photoshop 28.0 could not update." */
export function failedSentence(j: Pick<Job, "kind" | "name" | "toVersion" | "fromVersion">): string {
  const v = j.kind === "uninstall" ? (j.fromVersion ?? j.toVersion) : j.toVersion;
  return `${j.name} ${v} ${COULD_NOT[j.kind]}.`;
}

/** Wording of the "name needs" sentence of the low disk dialog. */
export function lowDiskWords(names: string[], action: "install" | "update"): { name: string; eyebrow: string; needs: string } {
  if (names.length > 2) {
    const name = `${names.length} ${action === "update" ? "updates" : "installs"}`;
    return { name, eyebrow: `Before these ${name}`, needs: "need" };
  }
  const name = nameList(names);
  return {
    name,
    eyebrow: `Before ${action === "update" ? "updating" : "installing"} ${name}`,
    needs: names.length > 1 ? "need" : "needs",
  };
}

/** The "Time left" figure: the estimate, or why there is none. */
export function timeLeft(eta: number | null, downloading: boolean, approx: (s: number) => string): string {
  if (eta != null) return approx(eta);
  return downloading ? "Working it out" : "Nothing downloading";
}

/** No apps found at all, for example a first run on a PC with none of the supported apps. */
export function isEmptyLibrary(appCount: number): boolean {
  return appCount === 0;
}

/** Every source switched off in Settings. */
export function allSourcesOff(sources: Settings["sources"]): boolean {
  return Object.values(sources).every((on) => !on);
}
