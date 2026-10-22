// User-level actions: check disk space first, then call the backend.
import { call, send } from "./api";
import { driveOf, nameList } from "./format";
import { router, appHref } from "./router.svelte";
import { store } from "./store.svelte";
import type { AppEntry, CatalogEntry, Source } from "./types";
import { ui } from "./ui.svelte";
import { cmpVersion } from "./format";

interface SpaceItem {
  name: string;
  bytes: number;
  dir: string | null;
  source: Source;
  /** a new install can move to another drive, an update of an installed app cannot */
  relocatable: boolean;
}

function dirFor(source: Source): string | null {
  if (source === "adobe") return store.settings.adobeInstallDir || null;
  if (source === "blender") return store.settings.blenderInstallDir || null;
  return null;
}

function guard(items: SpaceItem[], go: () => void) {
  const need = new Map<string, SpaceItem[]>();
  for (const it of items) {
    const drive = driveOf(it.dir ?? dirFor(it.source)) ?? driveOf(store.status.disks[0]?.path);
    if (!drive) continue;
    need.set(drive, [...(need.get(drive) ?? []), it]);
  }
  let worst: { drive: string; needed: number; free: number; list: SpaceItem[] } | null = null;
  for (const [drive, list] of need) {
    const free = store.freeOn(drive);
    if (free == null) continue;
    const needed = list.reduce((n, i) => n + i.bytes, 0);
    if (free < needed && (!worst || needed - free > worst.needed - worst.free)) worst = { drive, needed, free, list };
  }
  if (!worst) return go();
  const w = worst;
  const names = w.list.map((i) => i.name);
  const short = w.needed - w.free;
  const canRelocate = w.list.every((i) => i.relocatable && (i.source === "adobe" || i.source === "blender"));
  const other = canRelocate
    ? store.status.disks
        .map((d) => ({ drive: driveOf(d.path)!, free: d.free }))
        .filter((d) => d.drive !== w.drive && d.free >= w.needed)
        .sort((a, b) => b.free - a.free)[0] ?? null
    : null;
  ui.openDialog({
    type: "lowdisk",
    name: names.length > 2 ? `${names.length} updates` : nameList(names),
    drive: w.drive,
    needed: w.needed,
    free: w.free,
    cacheSize: store.status.cacheSize,
    relocate: other,
    proceed: go,
    onClearCache: async () => {
      if (store.status.cacheSize >= short) {
        await call("clear_cache", {});
        ui.closeDialog();
        go();
      }
    },
    onRelocate: () => {
      if (!other) return;
      const patch: Record<string, string> = {};
      const src = w.list[0].source;
      if (src === "adobe") patch.adobeInstallDir = `${other.drive.replace(/[\\/]+$/, "")}\\Adobe`;
      if (src === "blender") patch.blenderInstallDir = `${other.drive.replace(/[\\/]+$/, "")}\\Blender Foundation`;
      store.patchSettings(patch);
      ui.closeDialog();
      go();
    },
  });
}

const upd = (a: AppEntry): SpaceItem => ({ name: a.name, bytes: a.updateSize ?? 0, dir: a.installDir, source: a.source, relocatable: false });

export function updateApps(ids: string[]) {
  const list = ids.map((id) => store.app(id)).filter((a): a is AppEntry => !!a && (a.status === "update"));
  if (!list.length) return;
  guard(list.map(upd), () => void send("update_apps", { appIds: list.map((a) => a.id) }));
}
export function updateOne(id: string) {
  updateApps([id]);
}
export function updateAll() {
  if (!store.updates.length) return;
  guard(store.updates.map(upd), () => void send("update_all", {}));
}
export function installApp(entry: CatalogEntry, version: string | null = null) {
  guard(
    [{ name: entry.name, bytes: entry.size ?? 0, dir: null, source: entry.source, relocatable: true }],
    () => void send("install_app", { id: entry.id, version }),
  );
}
export function installVersion(app: AppEntry, version: string, size: number | null) {
  const back = cmpVersion(version, app.version) < 0;
  guard(
    [{ name: `${app.name} ${version}`, bytes: back && store.settings.keepPreviousVersions ? 0 : (size ?? 0), dir: app.installDir, source: app.source, relocatable: app.source === "blender" }],
    () => void send("install_version", { appId: app.id, version }),
  );
}
export function repairApp(id: string) {
  void send("repair_app", { appId: id });
}
export function askUninstall(id: string) {
  ui.openDialog({ type: "uninstall", appId: id });
}
export function openApp(id: string) {
  void send("open_app", { appId: id });
}
export function showInFolder(id: string) {
  void send("show_in_folder", { appId: id });
}
export function checkNow() {
  if (store.status.checking) return;
  void send("check_now", {});
}
export function togglePauseAll() {
  const anyRunning = store.jobs.some((j) => j.state === "downloading" || j.state === "installing" || j.state === "waiting");
  void send(anyRunning ? "pause_all" : "resume_all", {});
}
export function isPaused() {
  return store.openJobs.length > 0 && store.openJobs.every((j) => j.state === "paused" || j.state === "blocked");
}

/** Menu pick for an app's "..." menu */
export function runAppMenu(app: AppEntry, id: string) {
  switch (id) {
    case "notes":
      router.go(appHref(app.id));
      break;
    case "version":
      ui.pickVersion = true;
      router.go(appHref(app.id));
      break;
    case "repair":
      repairApp(app.id);
      break;
    case "folder":
      showInFolder(app.id);
      break;
    case "uninstall":
      askUninstall(app.id);
      break;
  }
}
