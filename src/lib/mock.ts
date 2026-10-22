// In-memory mock backend for browser preview (npm run dev).
// Dev switch: ?state=ready (default) | current | repair | updating | installing | first | offline | blocked | failed | lowdisk | ccdown | signedout
import type { Commands } from "./commands";
import type {
  ActivityEntry,
  AppEntry,
  CatalogEntry,
  EventMap,
  Job,
  JobError,
  JobKind,
  ServiceInfo,
  Settings,
  Snapshot,
  Status,
  VersionInfo,
} from "./types";

const MODE = new URLSearchParams(location.search).get("state") ?? "ready";
const GB = 1e9;
const MB = 1e6;
const SIM = 6; // simulated bytes advance faster than the speed shown, so jobs finish in seconds
const MIN = 60_000;
const DAY = 86_400_000;

// ---------- event bus ----------
type Cb = (p: never) => void;
const listeners = new Map<string, Set<Cb>>();
export function listen<K extends keyof EventMap>(event: K, cb: (p: EventMap[K]) => void): () => void {
  let set = listeners.get(event);
  if (!set) listeners.set(event, (set = new Set()));
  set.add(cb as Cb);
  return () => set!.delete(cb as Cb);
}
function emit<K extends keyof EventMap>(event: K, payload: EventMap[K]) {
  const copy = JSON.parse(JSON.stringify(payload));
  listeners.get(event)?.forEach((cb) => (cb as (p: unknown) => void)(copy));
}
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v));

// ---------- data ----------
const iso = (msAgo: number) => new Date(Date.now() - msAgo).toISOString();

function app(p: Partial<AppEntry> & Pick<AppEntry, "id" | "source" | "name" | "code" | "fg" | "bg" | "version" | "group">): AppEntry {
  return {
    iconUrl: null,
    latest: null,
    status: "current",
    updateSize: null,
    sizeOnDisk: null,
    installDir: null,
    updatedAt: null,
    tagline: null,
    running: false,
    canUninstall: true,
    canRollback: true,
    channel: null,
    note: null,
    jobId: null,
    ...p,
  };
}

const ADOBE = "C:\\Program Files\\Adobe\\";
function seedApps(): AppEntry[] {
  const upd = MODE !== "current" && MODE !== "repair";
  const list: AppEntry[] = [
    app({
      id: "adobe:SBSTP", source: "adobe", group: "Adobe", name: "Substance 3D Painter", code: "Pt", fg: "#FF5C6C", bg: "#2A0A10",
      version: "12.1.5", latest: "12.2.0", status: "update", updateSize: 2.53 * GB, sizeOnDisk: 4.1 * GB,
      installDir: ADOBE + "Adobe Substance 3D Painter", updatedAt: iso(40 * DAY), tagline: "Paint and texture 3D models",
    }),
    app({
      id: "adobe:SBSTD", source: "adobe", group: "Adobe", name: "Substance 3D Designer", code: "Ds", fg: "#F0A33A", bg: "#2B1A05",
      version: "16.0.4", latest: "16.0.7", status: "update", updateSize: 1.1 * GB, sizeOnDisk: 3.2 * GB,
      installDir: ADOBE + "Adobe Substance 3D Designer", updatedAt: iso(21 * DAY), tagline: "Build node based materials",
    }),
    app({
      id: "adobe:PHSP", source: "adobe", group: "Adobe", name: "Photoshop", code: "Ps", fg: "#31A8FF", bg: "#001E36",
      version: "27.2", latest: null, status: "missing", sizeOnDisk: null, installDir: ADOBE + "Adobe Photoshop 2026",
      updatedAt: iso(70 * DAY), tagline: "Edit and composite images", note: "Repair reinstalls it",
    }),
    app({
      id: "adobe:APRO", source: "adobe", group: "Adobe", name: "Acrobat", code: "Ac", fg: "#FF6A5C", bg: "#2B0705",
      version: "26.001.20437", latest: "26.001.20437", status: "current", sizeOnDisk: 1.6 * GB,
      installDir: ADOBE + "Acrobat DC", updatedAt: iso(9 * DAY), tagline: "Read, edit and sign PDFs",
    }),
    app({
      id: "craft:artcraft-launcher", source: "craft", group: "Craft apps", name: "ArtCraft Launcher", code: "Al", fg: "#D9B26A", bg: "#2A2005",
      version: "0.4.1", latest: "0.6.1", status: "update", updateSize: 84 * MB, sizeOnDisk: 310 * MB,
      installDir: "C:\\Users\\Cloud\\AppData\\Local\\ArtCraft\\Launcher", updatedAt: iso(12 * DAY), tagline: "Opens and updates the Craft apps",
    }),
    app({
      id: "craft:photocraft", source: "craft", group: "Craft apps", name: "PhotoCraft", code: "Ph", fg: "#7C93D6", bg: "#101A33",
      version: "0.9.0", latest: "0.12.0", status: "update", updateSize: 212 * MB, sizeOnDisk: 780 * MB,
      installDir: "C:\\Users\\Cloud\\AppData\\Local\\ArtCraft\\PhotoCraft", updatedAt: iso(18 * DAY), tagline: "Photo editing",
    }),
    app({
      id: "craft:figcraft", source: "craft", group: "Craft apps", name: "FigCraft", code: "Fg", fg: "#D98BA6", bg: "#2E1019",
      version: "0.6.0", latest: "0.9.0", status: "update", updateSize: 168 * MB, sizeOnDisk: 520 * MB,
      installDir: "C:\\Users\\Cloud\\AppData\\Local\\ArtCraft\\FigCraft", updatedAt: iso(30 * DAY), tagline: "Figures and layouts",
    }),
  ];
  const bl = (series: string, version: string, extra: Partial<AppEntry> = {}) =>
    app({
      id: `blender:${series}`, source: "blender", group: "Blender", name: `Blender ${series}`, code: "Bl", fg: "#EA7600", bg: "#2A1500",
      version, latest: version, status: "current", sizeOnDisk: 1.2 * GB, channel: "stable",
      installDir: `D:\\Blender\\Blender ${series}`, updatedAt: iso(20 * DAY), tagline: "3D creation suite", ...extra,
    });
  list.push(
    bl("4.2", "4.2.9", { channel: "lts", note: "Long-term support series", updatedAt: iso(120 * DAY) }),
    bl("4.4", "4.4.3", { updatedAt: iso(90 * DAY) }),
    bl("4.5", "4.5.5", {
      latest: "4.5.6", status: "update", updateSize: 328 * MB, note: "Bug-fix update, replaces 4.5.5", updatedAt: iso(45 * DAY),
    }),
    bl("5.0", "5.0.1", { updatedAt: iso(75 * DAY) }),
    bl("5.1", "5.1.2", { updatedAt: iso(33 * DAY) }),
    bl("5.2", "5.2.2", { updatedAt: iso(2 * DAY) }),
  );
  list.push(
    app({
      id: "tools:obs", source: "tools", group: "More apps", name: "OBS Studio", code: "Ob", fg: "#E4E4EC", bg: "#1E1E27",
      version: "32.2.1", latest: "32.2.2", status: "update", updateSize: 152 * MB, sizeOnDisk: 780 * MB,
      installDir: "C:\\Program Files\\obs-studio", updatedAt: iso(26 * DAY), tagline: "Recording and live streaming",
    }),
    app({
      id: "other:7zip.7zip", source: "other", group: "Other apps", name: "7-Zip", code: "7z", fg: "#E4E4EC", bg: "#24242C",
      version: "26.04", latest: "26.05", status: "update", sizeOnDisk: 6 * MB,
      installDir: "C:\\Program Files\\7-Zip", updatedAt: iso(40 * DAY), tagline: "A file archiver with a high compression ratio.",
    }),
    app({
      id: "other:VideoLAN.VLC", source: "other", group: "Other apps", name: "VLC media player", code: "Vm", fg: "#E3C26F", bg: "#2B2311",
      version: "3.0.24", latest: "3.0.24", status: "current", sizeOnDisk: 180 * MB, canRollback: true,
      installDir: "C:\\Program Files\\VideoLAN\\VLC", updatedAt: iso(90 * DAY), tagline: "A free and open source multimedia player",
    }),
    app({
      id: "tools:krita", source: "tools", group: "More apps", name: "Krita", code: "Kr", fg: "#E38BC4", bg: "#2A1222",
      version: "6.0.4.1", latest: "6.0.4.1", status: "current", sizeOnDisk: 1.1 * GB,
      installDir: "C:\\Program Files\\Krita (x64)", updatedAt: iso(12 * DAY), tagline: "Digital painting and illustration",
    }),
    app({
      id: "tools:freecad:1.1", source: "tools", group: "More apps", name: "FreeCAD 1.1", code: "Fc", fg: "#E8736A", bg: "#2C1311",
      version: "1.1.3", latest: "1.1.4", status: "update", updateSize: 495 * MB, sizeOnDisk: 2.4 * GB,
      installDir: "C:\\Users\\Cloud\\AppData\\Local\\Programs\\FreeCAD 1.1", updatedAt: iso(60 * DAY), tagline: "Parametric 3D CAD modeler",
    }),
    app({
      id: "tools:godot:4.7.1", source: "tools", group: "More apps", name: "Godot 4.7.1", code: "Gd", fg: "#6FB1E3", bg: "#0F2131",
      version: "4.7.1", latest: "4.7.2", status: "update", updateSize: 82 * MB, sizeOnDisk: 190 * MB, note: "4.7.2 installs next to 4.7.1, your 4.7.1 stays",
      installDir: "C:\\Users\\Cloud\\AppData\\Local\\Programs\\Godot\\Godot 4.7.1", updatedAt: iso(100 * DAY), tagline: "Game engine and editor",
    }),
    app({
      id: "tools:unity:6000.3.20f1", source: "tools", group: "More apps", name: "Unity 6000.3.20f1", code: "Un", fg: "#F0F0F2", bg: "#2A2F36",
      version: "6000.3.20f1", latest: "6000.3.27f1", status: "update", updateSize: 3.94 * GB, sizeOnDisk: 7.9 * GB, note: "6000.3.27f1 installs next to 6000.3.20f1, your 6000.3.20f1 stays",
      installDir: "C:\\Program Files\\Unity\\Hub\\Editor\\6000.3.20f1", updatedAt: iso(40 * DAY), tagline: "Game engine and real-time 3D",
    }),
  );
  if (MODE === "current") {
    const ps = list.find((x) => x.id === "adobe:PHSP")!;
    ps.status = "current";
    ps.note = null;
    ps.latest = ps.version;
    ps.sizeOnDisk = 5.8 * GB;
  }
  if (!upd) {
    for (const a of list) {
      if (a.status === "update") {
        a.version = a.latest ?? a.version;
        a.status = "current";
        a.updateSize = null;
        a.note = (a.source === "blender" && a.channel !== "lts") || a.source === "tools" ? null : a.note;
      }
    }
  }
  return list;
}

function c(
  id: string, source: CatalogEntry["source"], name: string, code: string, fg: string, bg: string, version: string, size: number | null,
  tagline: string, group: string, extra: Partial<CatalogEntry> = {},
): CatalogEntry {
  return { id, source, name, code, fg, bg, iconUrl: null, version, size, tagline, group, prerelease: false, installed: false, pickVersion: false, ...extra };
}
// Adobe and Blender entries offer a version picker; Craft apps only ship the newest build.
const AD = { pickVersion: true };
const CATALOG: CatalogEntry[] = [
  c("adobe:PHSP", "adobe", "Photoshop", "Ps", "#31A8FF", "#001E36", "27.11", 2.9 * GB, "Edit and composite images", "Adobe", AD),
  c("adobe:ILST", "adobe", "Illustrator", "Ai", "#FF9A00", "#330000", "30.8.2", 2.1 * GB, "Vector graphics and logos", "Adobe", AD),
  c("adobe:AEFT", "adobe", "After Effects", "Ae", "#D291FF", "#1F0040", "26.5", 1.6 * GB, "Motion graphics and effects", "Adobe", AD),
  c("adobe:PPRO", "adobe", "Premiere Pro", "Pr", "#9999FF", "#00005B", "26.5.2", 2.6 * GB, "Video editing", "Adobe", AD),
  c("adobe:SBSTP", "adobe", "Substance 3D Painter", "Pt", "#FF5C6C", "#2A0A10", "12.2.0", 2.53 * GB, "Paint and texture 3D models", "Adobe", AD),
  c("adobe:SBSTD", "adobe", "Substance 3D Designer", "Ds", "#F0A33A", "#2B1A05", "16.0.7", 1.1 * GB, "Build node based materials", "Adobe", AD),
  c("adobe:SBSTS", "adobe", "Substance 3D Sampler", "Sa", "#3FD9C6", "#062420", "6.0.4", 1.6 * GB, "Create materials from photos", "Adobe", AD),
  c("adobe:STGR", "adobe", "Substance 3D Stager", "St", "#C08BFF", "#1D0A33", "3.1.9", 2.8 * GB, "Stage and render 3D scenes", "Adobe", AD),
  c("adobe:KBRG", "adobe", "Bridge", "Br", "#B4A5FF", "#16123A", "16.0.8", 1.1 * GB, "Browse and organise assets", "Adobe", AD),
  c("adobe:AME", "adobe", "Media Encoder", "Me", "#8FB8FF", "#00163D", "26.5.2", 1.2 * GB, "Encode and export media", "Adobe", AD),
  c("adobe:APRO", "adobe", "Acrobat", "Ac", "#FF6A5C", "#2B0705", "26.001.20437", 1.6 * GB, "Read, edit and sign PDFs", "Adobe", AD),
  c("craft:artcraft-launcher", "craft", "ArtCraft Launcher", "Al", "#D9B26A", "#2A2005", "0.6.1", 84 * MB, "Opens and updates the Craft apps", "Craft apps"),
  c("craft:photocraft", "craft", "PhotoCraft", "Ph", "#7C93D6", "#101A33", "0.12.0", 212 * MB, "Photo editing", "Craft apps"),
  c("craft:figcraft", "craft", "FigCraft", "Fg", "#D98BA6", "#2E1019", "0.9.0", 168 * MB, "Figures and layouts", "Craft apps"),
  c("craft:vectorcraft", "craft", "VectorCraft", "Vc", "#D9B26A", "#2A2005", "0.14.0", 160 * MB, "Vector drawing", "Craft apps"),
  c("craft:filmcraft", "craft", "FilmCraft", "Fc", "#D9706A", "#2E100E", "0.10.0", 240 * MB, "Video editing", "Craft apps"),
  c("craft:lightcraft", "craft", "LightCraft", "Lc", "#7FB77E", "#0F2410", "0.11.0", 190 * MB, "Photo library", "Craft apps"),
  c("craft:pdfcraft", "craft", "PdfCraft", "Pd", "#A98BD8", "#1D1030", "0.11.0", 120 * MB, "PDF tools", "Craft apps"),
  c("craft:effectcraft", "craft", "EffectCraft", "Ec", "#7C93D6", "#101A33", "0.14.0", 210 * MB, "Visual effects", "Craft apps"),
  c("craft:designcraft", "craft", "DesignCraft", "Dc", "#D98BA6", "#2E1019", "0.11.0", 175 * MB, "Layout and design", "Craft apps"),
  c("craft:wordcraft", "craft", "WordCraft", "Wc", "#A0A0A0", "#1F1F1F", "0.9.0", 98 * MB, "Documents", "Craft apps"),
  c("craft:soundcraft", "craft", "SoundCraft", "Sc", "#7FB77E", "#0F2410", "0.11.0", 140 * MB, "Audio editing", "Craft apps"),
  c("craft:deckcraft", "craft", "DeckCraft", "Dk", "#D9B26A", "#2A2005", "0.9.0", 110 * MB, "Presentations", "Craft apps"),
  c("craft:cadcraft", "craft", "CADCraft", "Cd", "#7C93D6", "#101A33", "0.10.0", 260 * MB, "Technical drawing", "Craft apps"),
  c("craft:gridcraft", "craft", "GridCraft", "Gc", "#D9706A", "#2E100E", "0.10.0", 90 * MB, "Spreadsheets", "Craft apps"),
  c("craft:artcraft", "craft", "ArtCraft", "Ar", "#A98BD8", "#1D1030", "0.41.0", 340 * MB, "Painting and illustration", "Craft apps"),
  c("craft:sculptcraft", "craft", "SculptCraft", "Sk", "#D9B26A", "#2A2005", "0.3.0", 300 * MB, "Digital sculpting", "Craft apps", { prerelease: true }),
  c("tools:obs", "tools", "OBS Studio", "Ob", "#E4E4EC", "#1E1E27", "32.2.2", 152 * MB, "Recording and live streaming", "More apps", AD),
  c("tools:krita", "tools", "Krita", "Kr", "#E38BC4", "#2A1222", "6.0.4.1", 167 * MB, "Digital painting and illustration", "More apps", AD),
  c("tools:freecad", "tools", "FreeCAD", "Fc", "#E8736A", "#2C1311", "1.1.4", 495 * MB, "Parametric 3D CAD modeler", "More apps", AD),
  c("tools:godot", "tools", "Godot", "Gd", "#6FB1E3", "#0F2131", "4.7.2", 82 * MB, "Game engine and editor", "More apps", AD),
  c("tools:unity", "tools", "Unity Editor", "Un", "#F0F0F2", "#2A2F36", "6000.3.27f1", 3.94 * GB, "Game engine and real-time 3D", "More apps", AD),
  c("other:7zip.7zip", "other", "7-Zip", "7z", "#E4E4EC", "#24242C", "26.05", null, "A file archiver with a high compression ratio.", "Other apps", AD),
  c("other:VideoLAN.VLC", "other", "VLC media player", "Vm", "#E3C26F", "#2B2311", "3.0.24", null, "A free and open source multimedia player", "Other apps", AD),
  c("blender:5.2", "blender", "Blender 5.2", "Bl", "#EA7600", "#2A1500", "5.2.2", 390 * MB, "Latest stable release", "Blender"),
  c("blender:5.3-beta", "blender", "Blender 5.3", "Bl", "#EA7600", "#2A1500", "5.3.0 Beta 873fa96", 392 * MB, "Alpha, beta and release candidate builds", "Blender", { prerelease: true, pickVersion: true }),
  c("blender:3.6", "blender", "Blender 3.6 LTS", "Bl", "#EA7600", "#2A1500", "3.6.23", 318 * MB, "Long-term support release", "Blender"),
  c("blender:older", "blender", "Older Blender versions", "Bl", "#EA7600", "#2A1500", "Choose a version", null, "Any release back to 2.93", "Blender", { pickVersion: true }),
];
/** Winget catalog packages a Get apps search finds (Other apps). */
const OTHER_SEARCH: CatalogEntry[] = [
  c("other:Notepad++.Notepad++", "other", "Notepad++", "No", "#9FD69B", "#14261A", "8.9.8.1", null, "Notepad++.Notepad++", "Other apps", AD),
  c("other:Mozilla.Firefox", "other", "Mozilla Firefox (en-US)", "Mf", "#E8736A", "#2C1311", "157.0.1", null, "Mozilla.Firefox", "Other apps", AD),
  c("other:7zip.7zip", "other", "7-Zip", "7z", "#E4E4EC", "#24242C", "26.05", null, "7zip.7zip", "Other apps", AD),
];
/** Releases of the "More apps" entries (newest first), as the backend lists them. */
const TOOLS_RELEASES: Record<string, { v: string; pre?: boolean; size: number; notes: string[] }[]> = {
  obs: [
    { v: "33.0.0-rc3", pre: true, size: 186 * MB, notes: ["Fixed a crash when switching scene collections", "Improved NVENC rate control"] },
    { v: "32.2.2", size: 152 * MB, notes: ["Fixed an issue where plugins might not load properly on the first start after updating OBS on Windows"] },
    { v: "32.2.1", size: 152 * MB, notes: ["Fixed game capture failing after updating OBS if the previous hook was still in use"] },
    { v: "32.2.0", size: 151 * MB, notes: ["Added a new audio monitoring device selector", "Updated the NVIDIA SDK"] },
  ],
  krita: [
    { v: "6.0.4.1", size: 167 * MB, notes: ["Fix a crash when opening the brush editor"] },
    { v: "6.0.4", size: 167 * MB, notes: ["No change list is published for this release"] },
    { v: "5.3.4.1", size: 160 * MB, notes: ["Fix layer thumbnails scaling when using display scaling", "Fix opening large exr files"] },
  ],
  freecad: [
    { v: "26.3rc2", pre: true, size: 416 * MB, notes: ["Fixes made since the first release candidate"] },
    { v: "1.1.4", size: 495 * MB, notes: ["Fix handling of crafted FCStd files", "Improve performance of the sketcher"] },
    { v: "1.1.3", size: 495 * MB, notes: ["Maintenance release"] },
    { v: "1.1.2", size: 495 * MB, notes: ["Maintenance release"] },
  ],
  godot: [
    { v: "4.8-beta1", pre: true, size: 62 * MB, notes: ["Godot 4.8 beta 1 is ready for testing"] },
    { v: "4.7.2", size: 82 * MB, notes: ["Godot 4.7.2 is a maintenance release addressing stability and usability issues"] },
    { v: "4.7.1", size: 82 * MB, notes: ["Godot 4.7.1 is a maintenance release"] },
    { v: "4.7", size: 81 * MB, notes: ["Godot 4.7 is a new feature release"] },
    { v: "3.6.3", size: 34 * MB, notes: ["Godot 3.6.3 is a maintenance release"] },
  ],
  unity: [
    { v: "6000.3.27f1", size: 3.94 * GB, notes: ["Asset Import: Improved the security of model import by updating the bundled Autodesk FBX SDK", "Graphics: Added Graphics.disableDepthAndStencilBuffers"] },
    { v: "6000.3.26f1", size: 3.94 * GB, notes: ["Editor: Fixed a crash when closing the Profiler"] },
    { v: "6000.0.85f1", size: 3.7 * GB, notes: ["Scripting: Fixed a crash on domain reload"] },
    { v: "2022.3.62f3", size: 3.3 * GB, notes: ["Physics: Fixed a rare crash in the broadphase"] },
    { v: "6000.7.0b6", pre: true, size: 4.1 * GB, notes: ["Beta build"] },
  ],
};
function toolsVersions(key: string, installedVersions: string[], within?: string): VersionInfo[] {
  const rel = TOOLS_RELEASES[key] ?? [];
  const series = (v: string) => v.split(".").slice(0, 2).join(".");
  const list = within ? rel.filter((r) => series(r.v) === series(within)) : rel;
  const newest = list.find((r) => !r.pre)?.v;
  return list.map((r, i) => ({
    version: r.v,
    date: iso((i * 19 + 3) * DAY).slice(0, 10),
    notes: r.notes,
    notesUrl: null,
    size: r.size,
    installed: installedVersions.includes(r.v),
    latest: r.v === newest,
    prerelease: !!r.pre,
  }));
}
/** FreeCAD installs one copy per series, Godot and Unity one per version; OBS Studio and Krita one in all. */
function toolsId(key: string, version: string): string {
  if (key === "freecad") return `tools:freecad:${/^\d+\.\d+/.exec(version)?.[0] ?? version}`;
  return key === "godot" || key === "unity" ? `tools:${key}:${version}` : `tools:${key}`;
}

/** Catalog as the backend would send it: installed flags follow the installed list. */
function catalogNow(): CatalogEntry[] {
  return CATALOG.map((e) => ({ ...e, installed: apps.some((a) => a.id === e.id && !(a.jobId && a.status !== "current" && !a.updatedAt)) }));
}

/** Pre-release builds the beta entry offers, newest first. */
const BLENDER_PRE = [
  "5.4.0 Alpha fbe82a4",
  "5.3.0 Beta 873fa96",
  "5.3.0 Beta 5be0e41",
  "5.3.0 Alpha 20d1f7c",
  "5.2.0 Release Candidate a41b9c3",
  "5.2.0 Beta d09e3a2",
  "5.2.0 Alpha 7c13e58",
  "5.1.0 Release Candidate 3e8f0b6",
];
const BLENDER_RELEASES = ["5.4.0 Alpha fbe82a4", "5.3.0 Beta 873fa96", "5.2.2", "5.2.1", "5.2.0", "5.1.2", "5.1.1", "5.0.1", "4.5.6", "4.5.5", "4.4.3", "4.2.9"];
const BLENDER_OLDER = ["5.1.2", "5.0.1", "4.5.6", "4.4.3", "4.3.2", "4.2.9", "4.1.1", "4.0.2", "3.6.23", "3.3.21", "2.93.18"];

let apps: AppEntry[] = [];
let jobs: Job[] = [];
let activity: ActivityEntry[] = [];
let settings: Settings = {
  checkInterval: 4,
  autoMode: "install",
  windowStart: "02:00",
  windowEnd: "06:00",
  exceptions: { "craft:photocraft": "notify", "blender:4.5": "off" },
  keepPreviousVersions: true,
  bandwidthLimit: 40 * MB,
  bandwidthLimitEnabled: true,
  launchAtStartup: true,
  startMinimized: false,
  closeToTray: true,
  notifyUpdates: true,
  notifyFinished: true,
  notifyFailed: true,
  language: "en_US",
  adobeInstallDir: "C:\\Program Files\\Adobe",
  blenderInstallDir: "D:\\Blender",
  godotInstallDir: "C:\\Users\\Cloud\\AppData\\Local\\Programs\\Godot",
  blenderChannel: "beta",
  blenderKeepPatchVersions: false,
  craftPrereleases: false,
  sources: { adobe: true, craft: true, blender: true, tools: true, other: true },
  parallelDownloads: 3,
  silentElevation: false,
  uiZoom: 1,
};
let status: Status = {
  online: true,
  checking: false,
  lastCheck: iso(3 * MIN),
  ccRunning: true,
  ccInstalled: true,
  signedIn: true,
  disks: [
    { path: "C:\\", free: 48 * GB, total: 476 * GB },
    { path: "D:\\", free: 412 * GB, total: 1000 * GB },
  ],
  cacheSize: 2.8 * GB,
  scanStep: null,
  firstScan: false,
  appVersion: "1.0.0",
  selfUpdate: MODE === "ready" ? "1.1.0" : null,
};

let services: ServiceInfo[] = [
  { name: "Adobe Desktop Service", displayName: "Adobe Desktop Service", description: "Sign-in, licensing and installs", kind: "service", running: true, cpu: 0.6, memory: 214 * MB, required: true, safeToStop: false },
  { name: "AdobeIPCBroker", displayName: "AdobeIPCBroker", description: "Connects Adobe apps to your sign-in", kind: "process", running: true, cpu: 0.1, memory: 18 * MB, required: true, safeToStop: false },
  { name: "Creative Cloud", displayName: "Creative Cloud", description: "The Creative Cloud window, not needed with this app", kind: "process", running: true, cpu: 0.0, memory: 186 * MB, required: false, safeToStop: true },
  { name: "CoreSync", displayName: "CoreSync", description: "Syncs Creative Cloud files", kind: "process", running: true, cpu: 0.4, memory: 128 * MB, required: false, safeToStop: true },
  { name: "CCXProcess", displayName: "CCXProcess", description: "Extensions and in-app content", kind: "process", running: true, cpu: 0.3, memory: 96 * MB, required: false, safeToStop: true },
  { name: "CCLibrary", displayName: "CCLibrary", description: "Syncs Libraries", kind: "process", running: true, cpu: 0.2, memory: 74 * MB, required: false, safeToStop: true },
  { name: "AGSService", displayName: "AGSService", description: "Genuine software check", kind: "service", running: true, cpu: 0.1, memory: 21 * MB, required: false, safeToStop: true },
  { name: "AdobeUpdateService", displayName: "AdobeUpdateService", description: "Adobe update checker, this app does its job", kind: "service", running: false, cpu: 0, memory: 0, required: false, safeToStop: true },
];

function pushActivity(kind: ActivityEntry["kind"], title: string, meta: string | null, appId: string | null, error: JobError | null = null, msAgo = 0) {
  const e: ActivityEntry = { id: "a" + Math.random().toString(36).slice(2, 8), time: iso(msAgo), kind, title, meta, appId, error };
  activity.unshift(e);
  emit("activity", e);
}

const ERR183: JobError = {
  code: "Error 183",
  message: "Files in use. Substance3DPainter.exe was running.",
  detail:
    "Error 183: files in use. Substance3DPainter.exe (PID 14212) was running.\nThe installer could not replace Substance3DPainter.dll and stopped.\nNothing was changed. 12.1.5 is still installed.\nLog: %LOCALAPPDATA%\\Temp\\CreativeCloud\\ACC\\install.log",
  logPath: "C:\\Users\\Cloud\\AppData\\Local\\Temp\\CreativeCloud\\ACC\\install.log",
};

function seedActivity() {
  const mk = (msAgo: number, kind: ActivityEntry["kind"], title: string, meta: string | null, appId: string | null = null, error: JobError | null = null) => ({
    id: "s" + msAgo, time: iso(msAgo), kind, title, meta, appId, error,
  });
  activity = [
    mk(5 * MIN, "check", "Checked for updates. 5 found.", null),
    mk(3 * 3600e3, "update", "Updated Blender 5.2.1 to 5.2.2", "340 MB, 52 s", "blender:5.2"),
    mk(1 * DAY + 2 * 3600e3, "rollback", "Rolled back Substance 3D Painter 12.1.6 to 12.1.5", "Kept version, no download", "adobe:SBSTP"),
    mk(1 * DAY + 3 * 3600e3, "update", "Updated Acrobat 26.001.20321 to 26.001.20437", "1.1 GB, 2 min 40 s", "adobe:APRO"),
    mk(9 * DAY, "install", "Installed Blender 5.1", "318 MB, 41 s", "blender:5.1"),
    mk(9 * DAY + 600e3, "update", "Updated PhotoCraft 0.8.0 to 0.9.0", "198 MB, 33 s", "craft:photocraft"),
    mk(12 * DAY, "service", "Stopped 4 background services", "505 MB freed"),
  ];
  if (MODE === "failed") {
    activity.unshift(mk(8 * MIN, "failure", "Substance 3D Painter 12.2.0 update failed", "Error 183", "adobe:SBSTP", ERR183));
  }
  if (MODE !== "current") {
    activity.splice(2, 0, mk(2 * DAY, "failure", "Substance 3D Designer 16.0.5 update failed", "Checksum mismatch", "adobe:SBSTD", {
      code: "Checksum mismatch", message: "The download was damaged.",
      detail: "Checksum mismatch for Designer_16.0.5.pak\nExpected 9f2c1e7a, got 77ab03d1.\nThe file was deleted and nothing was installed.\nLog: %LOCALAPPDATA%\\Temp\\CreativeCloud\\ACC\\install.log",
      logPath: null,
    }));
  }
}

// ---------- jobs ----------
let jobSeq = 1;
let failOnce = MODE === "failed";
const hidden = new Map<string, { resume: Job["state"] }>();
/** Jobs that stay in the installer step (?state=installing), to look at the disabled controls. */
const held = new Set<string>();

function activeState(s: Job["state"]) {
  return s === "downloading" || s === "installing" || s === "verifying";
}
function isOpen(s: Job["state"]) {
  return s === "waiting" || s === "paused" || s === "blocked" || activeState(s);
}

function makeJob(a: AppEntry, kind: JobKind, toVersion: string, total: number): Job {
  const id = "j" + jobSeq++;
  return {
    id, appId: a.id, name: a.name, code: a.code, fg: a.fg, bg: a.bg, iconUrl: a.iconUrl, kind,
    fromVersion: kind === "install" ? null : a.version, toVersion, state: "waiting",
    downloaded: 0, total, speed: 0, eta: null, progress: 0,
    step: "Waiting", blockedBy: [], error: null, startedAt: null, finishedAt: null,
  };
}

function enqueue(a: AppEntry, kind: JobKind, toVersion: string, total: number) {
  if (jobs.some((j) => j.appId === a.id && isOpen(j.state))) return null;
  const j = makeJob(a, kind, toVersion, total);
  jobs.push(j);
  a.jobId = j.id;
  syncApp(a, j);
  return j;
}

function syncApp(a: AppEntry, j: Job) {
  a.jobId = isOpen(j.state) ? j.id : null;
  switch (j.state) {
    case "waiting":
    case "paused":
      a.status = "queued";
      break;
    case "downloading":
    case "installing":
    case "verifying":
      a.status = "updating";
      break;
    case "blocked":
      a.status = "blocked";
      break;
    case "failed":
      a.status = "failed";
      break;
    case "canceled":
      a.status = baseStatus(a);
      break;
  }
}
function baseStatus(a: AppEntry): AppEntry["status"] {
  if (a.id === "adobe:PHSP" && a.version === "27.2") return "missing";
  return a.latest && a.latest !== a.version ? "update" : "current";
}

function downloadsRunning() {
  return jobs.filter((j) => j.state === "downloading").length;
}

function startMore() {
  const running = jobs.filter((j) => activeState(j.state)).length;
  let slots = settings.parallelDownloads - running;
  for (const j of jobs) {
    if (slots <= 0) break;
    if (j.state === "waiting") {
      j.state = j.total > 0 ? "downloading" : "installing";
      j.startedAt = new Date().toISOString();
      j.step = j.total > 0 ? "Downloading" : "Removing";
      slots--;
    }
  }
}

function finish(j: Job) {
  const a = apps.find((x) => x.id === j.appId);
  j.state = "done";
  j.progress = 1;
  j.speed = 0;
  j.eta = null;
  j.step = "Done";
  j.finishedAt = new Date().toISOString();
  const secs = j.startedAt ? Math.max(1, Math.round((Date.now() - +new Date(j.startedAt)) / 1000)) : 1;
  const sz = j.total > GB ? (j.total / GB).toFixed(1) + " GB" : Math.round(j.total / MB) + " MB";
  const t = secs < 60 ? `${secs} s` : `${Math.floor(secs / 60)} min ${secs % 60} s`;
  if (a) {
    a.jobId = null;
    if (j.kind === "uninstall") {
      apps = apps.filter((x) => x.id !== a.id);
      pushActivity("uninstall", `Uninstalled ${j.name} ${j.fromVersion ?? ""}`.trim(), null, null);
      emit("toast", { kind: "ok", text: `${j.name} was removed` });
    } else if (j.kind === "install" && !apps.some((x) => x.id === a.id)) {
      /* handled at creation */
    } else {
      a.version = j.toVersion;
      a.status = "current";
      a.updateSize = null;
      a.updatedAt = new Date().toISOString();
      a.running = false;
      if (a.source !== "blender" || a.latest === j.toVersion || !a.latest) a.latest = j.toVersion;
      else a.latest = j.toVersion;
      if (a.source === "blender" && a.channel !== "lts") a.note = null;
      const verb = j.kind === "rollback" ? "Rolled back" : j.kind === "repair" ? "Repaired" : j.kind === "install" ? "Installed" : "Updated";
      const title =
        j.kind === "update" || j.kind === "rollback"
          ? `${verb} ${j.name} ${j.fromVersion} to ${j.toVersion}`
          : `${verb} ${j.name} ${j.toVersion}`;
      pushActivity(j.kind === "repair" ? "install" : j.kind, title, j.kind === "rollback" && settings.keepPreviousVersions ? "Kept version, no download" : `${sz}, ${t}`, a.id);
      emit("toast", { kind: "ok", text: `${j.name} ${j.kind === "rollback" ? "rolled back to" : j.kind === "repair" ? "repaired at" : j.kind === "install" ? "installed at" : "updated to"} ${j.toVersion}`, appId: a.id });
    }
  }
}

function fail(j: Job, err: JobError) {
  j.state = "failed";
  j.error = err;
  j.speed = 0;
  j.eta = null;
  j.step = "Failed";
  j.finishedAt = new Date().toISOString();
  const a = apps.find((x) => x.id === j.appId);
  if (a) syncApp(a, j);
  pushActivity("failure", `${j.name} ${j.toVersion} ${j.kind === "install" ? "install" : "update"} failed`, err.code, j.appId, err);
  emit("toast", { kind: "err", text: `${j.name} update failed. ${err.message}`, appId: j.appId });
}

function tick(dt: number) {
  let changed = false;
  startMore();
  const dl = downloadsRunning();
  const share = dl ? (settings.bandwidthLimitEnabled && settings.bandwidthLimit > 0 ? settings.bandwidthLimit : 70 * MB) / dl : 0;
  for (const j of jobs) {
    const a = apps.find((x) => x.id === j.appId);
    if (j.state === "downloading") {
      j.speed = Math.max(2 * MB, share * (0.85 + Math.random() * 0.25));
      j.downloaded = Math.min(j.total, j.downloaded + j.speed * dt * SIM);
      j.eta = (j.total - j.downloaded) / (j.speed * SIM);
      j.progress = 0.8 * (j.downloaded / j.total);
      j.step = "Downloading";
      if (j.downloaded >= j.total) {
        j.state = "installing";
        j.speed = 0;
        j.eta = 8;
        j.step = "Installing, step 1 of 4";
      }
      changed = true;
    } else if (j.state === "installing" || j.state === "verifying") {
      if (held.has(j.id)) {
        if (a) syncApp(a, j);
        continue;
      }
      const rate = j.total > 0 ? 0.2 / 6 : 1 / 2; // seconds to finish the install part
      if (a?.running && j.total > 0 && MODE === "blocked" && j.progress >= 0.8) {
        j.state = "blocked";
        j.blockedBy = [a.id === "adobe:SBSTD" ? "Substance3DDesigner.exe" : "app.exe"];
        j.step = `Waiting for ${j.name} to close`;
        j.eta = null;
      } else if (failOnce && j.appId === "adobe:SBSTP" && j.progress >= 0.9) {
        failOnce = false;
        fail(j, ERR183);
      } else {
        j.progress = Math.min(1, j.progress + rate * dt);
        const step = Math.min(4, 1 + Math.floor(((j.progress - 0.8) / 0.2) * 4));
        j.step = j.total > 0 ? `Installing, step ${Math.max(1, step)} of 4` : "Removing";
        j.eta = Math.max(1, (1 - j.progress) / rate);
        if (j.progress >= 1) finish(j);
      }
      changed = true;
    }
    if (a) syncApp(a, j);
  }
  if (changed) {
    emit("jobs", jobs);
    emit("apps", apps);
  }
}

// ---------- scenarios ----------
function advance(seconds: number) {
  for (let i = 0; i < seconds * 4; i++) tick(0.25);
}

function setup() {
  apps = seedApps();
  seedActivity();
  if (MODE === "first") {
    const all = apps;
    apps = [];
    status.firstScan = true;
    status.lastCheck = null;
    status.scanStep = "Reading installed apps. 0 found.";
    let i = 0;
    const t = setInterval(() => {
      if (i < all.length) {
        apps.push(all[i++]);
        status.scanStep = `Reading installed apps. ${i} found.`;
      } else if (i === all.length) {
        status.scanStep = "Checking for updates";
        i++;
      } else {
        status.firstScan = false;
        status.scanStep = null;
        status.lastCheck = new Date().toISOString();
        clearInterval(t);
      }
      emit("apps", apps);
      emit("status", status);
    }, 1100);
  }
  if (MODE === "offline") {
    status.online = false;
    status.lastCheck = iso(95 * MIN);
  }
  if (MODE === "lowdisk") status.disks[0].free = 2.1 * GB;
  if (MODE === "ccdown") {
    status.ccRunning = false;
    services.forEach((s) => {
      if (s.required) {
        s.running = false;
        s.memory = 0;
        s.cpu = 0;
      }
    });
  }
  if (MODE === "signedout") status.signedIn = false;
  if (MODE === "updating") {
    for (const a of apps) if (a.status === "update") enqueue(a, "update", a.latest!, a.updateSize ?? 100 * MB);
    advance(2);
  }
  if (MODE === "blocked") {
    for (const id of ["adobe:SBSTP", "adobe:SBSTD", "craft:artcraft-launcher"]) {
      const a = apps.find((x) => x.id === id)!;
      enqueue(a, "update", a.latest!, a.updateSize!);
    }
    apps.find((x) => x.id === "adobe:SBSTD")!.running = true;
    advance(16);
  }
  if (MODE === "failed") {
    for (const id of ["adobe:SBSTP", "adobe:SBSTD", "craft:artcraft-launcher"]) {
      const a = apps.find((x) => x.id === id)!;
      enqueue(a, "update", a.latest!, a.updateSize!);
    }
    advance(40);
  }
}
setup();

setInterval(() => tick(0.25), 250);

// ---------- versions ----------
function prevVersion(v: string): string {
  const parts = v.split(".").map((x) => parseInt(x, 10));
  for (let i = parts.length - 1; i >= 0; i--) {
    if (parts[i] > 0) {
      parts[i]--;
      if (i === 0 && parts.length > 1) parts[1] = 9;
      return parts.slice(0, Math.max(i + 1, Math.min(parts.length, 2))).join(".");
    }
  }
  return v;
}
const NOTES = [
  ["Fixes a crash on startup with some GPU drivers", "Faster project loading on large files"],
  ["Improves brush response on high refresh rate displays", "Fixes missing fonts after waking from sleep"],
  ["Adds export presets for common formats", "Smaller memory use while idle", "Fixes an issue with pen tablets on Windows"],
  ["Stability fixes"],
];

/** Versions of an Other apps package: one change list from its manifest, one from its notes page, one with none. */
function otherVersions(id: string): VersionInfo[] {
  const have = apps.find((a) => a.id === id)?.version;
  const vs = id === "other:7zip.7zip" ? ["26.05", "26.04", "26.03"] : ["3.0.24", "3.0.23", "3.0.21"];
  return vs.map((v, i) => ({
    version: v,
    date: iso((i * 30 + 2) * DAY).slice(0, 10),
    notes: i === 2 ? ["No change list is published for this release"] : i === 1 ? ["See the full release notes for the changes"] : ["Improved code for unpacking archives", "Some bugs were fixed"],
    notesUrl: i === 2 ? null : "https://example.com/release-notes",
    size: null,
    installed: v === have,
    latest: i === 0,
    prerelease: false,
  }));
}

function versionsFor(a: AppEntry): VersionInfo[] {
  const list: VersionInfo[] = [];
  if (a.source === "other") return otherVersions(a.id);
  if (a.source === "tools") {
    const key = a.id.split(":")[1];
    return toolsVersions(key, [a.version], key === "freecad" || key === "godot" || key === "unity" ? a.version : undefined);
  }
  if (a.source === "blender") {
    BLENDER_RELEASES.forEach((v, i) => {
      const installed = apps.some((x) => x.source === "blender" && x.version === v);
      list.push({
        version: v,
        date: iso((i * 24 + 3) * DAY / 6).slice(0, 10),
        notes: NOTES[i % NOTES.length],
        notesUrl: `https://www.blender.org/download/releases/${v.split(".").slice(0, 2).join("-")}/`,
        size: (320 + i * 9) * MB,
        installed,
        latest: v === "5.2.2",
        prerelease: v.includes(" "),
      });
    });
    return list;
  }
  const latest = a.latest && a.latest !== a.version ? a.latest : null;
  let day = 1;
  if (latest) {
    list.push({ version: latest, date: iso(day * DAY).slice(0, 10), notes: NOTES[0], notesUrl: null, size: a.updateSize, installed: false, latest: true, prerelease: false });
  }
  list.push({ version: a.version, date: a.updatedAt, notes: NOTES[1], notesUrl: null, size: a.sizeOnDisk, installed: true, latest: !latest, prerelease: false });
  let v = a.version;
  for (let i = 0; i < 3; i++) {
    v = prevVersion(v);
    day += 18 + i * 6;
    list.push({ version: v, date: iso((day + 10) * DAY).slice(0, 10), notes: [], notesUrl: null, size: a.sizeOnDisk, installed: false, latest: false, prerelease: false });
  }
  return list;
}

// ---------- commands ----------
function reorderCmp(a: string, b: string) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) if ((pa[i] ?? 0) !== (pb[i] ?? 0)) return (pa[i] ?? 0) - (pb[i] ?? 0);
  return 0;
}

type H = { [K in keyof Commands]: (args: Commands[K]["args"]) => Commands[K]["ret"] | Promise<Commands[K]["ret"]> };

function findApp(id: string) {
  return apps.find((a) => a.id === id);
}

const handlers: H = {
  get_snapshot: () => clone<Snapshot>({ apps, jobs, settings, status }),
  async check_now() {
    status.checking = true;
    emit("status", status);
    await new Promise((r) => setTimeout(r, 1500));
    status.checking = false;
    if (status.online) {
      status.lastCheck = new Date().toISOString();
      const n = apps.filter((a) => a.status === "update").length;
      pushActivity("check", `Checked for updates. ${n} found.`, null, null);
    } else {
      emit("toast", { kind: "warn", text: "Still offline. Checking again when the connection is back." });
    }
    emit("status", status);
  },
  update_apps({ appIds }) {
    for (const id of appIds) {
      const a = findApp(id);
      if (a && a.status === "update") enqueue(a, "update", a.latest!, a.updateSize ?? 100 * MB);
    }
    emit("jobs", jobs);
    emit("apps", apps);
  },
  update_all() {
    handlers.update_apps({ appIds: apps.filter((a) => a.status === "update").map((a) => a.id) });
  },
  install_app({ id, version }) {
    let cat = CATALOG.find((x) => x.id === id);
    if (!cat) return;
    if (cat.source === "tools") {
      const key = cat.id.split(":")[1];
      const chosen = version ?? cat.version;
      const tid = toolsId(key, chosen);
      const nm = key === "freecad" ? `FreeCAD ${tid.split(":")[2]}` : key === "godot" ? `Godot ${chosen}` : key === "unity" ? `Unity ${chosen}` : cat.name;
      cat = { ...cat, id: tid, name: nm, version: chosen };
      version = chosen;
    }
    if (cat.id === "blender:older" || (cat.source === "blender" && cat.prerelease)) {
      // "Older Blender versions" and the pre-release builds install the chosen release as its own series entry
      const chosen = version ?? (cat.pickVersion && /^\d/.test(cat.version) ? cat.version : null);
      if (!chosen) return;
      const series = chosen.split(".").slice(0, 2).join(".");
      cat = { ...cat, id: `blender:${series}`, name: `Blender ${series}`, version: chosen, size: cat.size ?? 320 * MB, tagline: "3D creation suite" };
      version = chosen;
    }
    let a = findApp(cat.id);
    if (!a) {
      a = app({
        id: cat.id, source: cat.source, group: cat.group, name: cat.name, code: cat.code, fg: cat.fg, bg: cat.bg,
        version: version ?? cat.version, latest: version ?? cat.version, status: "queued", sizeOnDisk: cat.size, tagline: cat.tagline,
        channel: cat.prerelease ? "beta" : cat.source === "blender" ? "stable" : null,
        installDir: cat.source === "blender" ? `${settings.blenderInstallDir}\\${cat.name}` : cat.id.startsWith("tools:godot") ? `${settings.godotInstallDir}\\${cat.name}` : null,
        updatedAt: null,
      });
      apps.push(a);
    }
    enqueue(a, "install", version ?? cat.version, cat.size ?? 200 * MB);
    emit("apps", apps);
    emit("jobs", jobs);
  },
  install_version({ appId, version }) {
    const a = findApp(appId);
    if (!a) return;
    if (a.source === "tools" && a.id.split(":").length > 2 && toolsId(a.id.split(":")[1], version) !== a.id) {
      // a version of another series or build installs as a copy of its own
      handlers.install_app({ id: `tools:${a.id.split(":")[1]}`, version });
      return;
    }
    if (a.source === "blender" && version.split(".").slice(0, 2).join(".") !== a.id.split(":")[1]) {
      const series = version.split(".").slice(0, 2).join(".");
      handlers.install_app({ id: `blender:${series}`, version });
      if (!CATALOG.some((x) => x.id === `blender:${series}`)) {
        const na = app({
          id: `blender:${series}`, source: "blender", group: "Blender", name: `Blender ${series}`, code: "Bl", fg: "#EA7600", bg: "#2A1500",
          version, latest: version, status: "queued", channel: "stable", tagline: "3D creation suite", updatedAt: new Date().toISOString(),
        });
        apps.push(na);
        enqueue(na, "install", version, 330 * MB);
        emit("apps", apps);
        emit("jobs", jobs);
      }
      return;
    }
    const back = reorderCmp(version, a.version) < 0;
    enqueue(a, back ? "rollback" : "install", version, back && settings.keepPreviousVersions ? 0 : (a.updateSize ?? 600 * MB));
    emit("apps", apps);
    emit("jobs", jobs);
  },
  uninstall_app({ appId }) {
    const a = findApp(appId);
    if (!a) return;
    const j = enqueue(a, "uninstall", a.version, 0);
    if (j) j.fromVersion = a.version;
    emit("apps", apps);
    emit("jobs", jobs);
  },
  repair_app({ appId }) {
    const a = findApp(appId);
    if (!a) return;
    enqueue(a, "repair", a.version, 400 * MB);
    emit("apps", apps);
    emit("jobs", jobs);
  },
  open_app: () => {},
  show_in_folder: () => {},
  get_versions: ({ appId }) => {
    const a = findApp(appId);
    return a ? clone(versionsFor(a)) : [];
  },
  get_catalog: () => clone(catalogNow()),
  search_other_apps: ({ query }) =>
    OTHER_SEARCH.filter((e) => e.name.toLowerCase().includes(query.toLowerCase()) || e.id.toLowerCase().includes(query.toLowerCase())).map((e) => ({ ...e, installed: apps.some((a) => a.id === e.id) })),
  async catalog_versions({ id }) {
    await new Promise((r) => setTimeout(r, 420)); // the real list comes over the network
    if (id.startsWith("other:")) return otherVersions(id);
    const cat = CATALOG.find((x) => x.id === id);
    if (!cat) return [];
    if (cat.source === "tools") {
      const key = id.split(":")[1];
      return toolsVersions(key, apps.filter((x) => x.id.startsWith(`tools:${key}`)).map((x) => x.version));
    }
    if (id === "blender:older") {
      return BLENDER_OLDER.map((v, i) => ({
        version: v,
        date: iso((i * 61 + 40) * DAY).slice(0, 10),
        notes: NOTES[i % NOTES.length],
        notesUrl: `https://www.blender.org/download/releases/${v.split(".").slice(0, 2).join("-")}/`,
        size: (300 + i * 7) * MB,
        installed: apps.some((x) => x.source === "blender" && x.version === v),
        latest: false,
        prerelease: false,
      }));
    }
    if (cat.source === "blender" && cat.prerelease) {
      return BLENDER_PRE.map((v, i) => ({
        version: v,
        date: iso((i * 9 + 2) * DAY).slice(0, 10),
        notes: i < 4 ? NOTES[i % NOTES.length] : [],
        notesUrl: i < 4 ? `https://developer.blender.org/docs/release_notes/${v.split(".").slice(0, 2).join(".")}/` : null,
        size: (385 + ((i * 7) % 19)) * MB,
        installed: apps.some((x) => x.source === "blender" && x.version === v),
        latest: i === 0,
        prerelease: true,
      }));
    }
    const patch = /^(\d+\.\d+)\.(\d+)$/.exec(cat.version);
    if (cat.source === "blender" && patch) {
      // every bug-fix release of the series, newest first
      return Array.from({ length: Number(patch[2]) + 1 }, (_, i) => {
        const v = `${patch[1]}.${Number(patch[2]) - i}`;
        return {
          version: v,
          date: iso((i * 38 + 6) * DAY).slice(0, 10),
          notes: i < 3 ? NOTES[i % NOTES.length] : [],
          notesUrl: `https://www.blender.org/download/releases/${patch[1].replace(".", "-")}/`,
          size: cat.size ? Math.round(cat.size * (1 - i * 0.01)) : null,
          installed: apps.some((x) => x.source === "blender" && x.version === v),
          latest: i === 0,
          prerelease: false,
        };
      });
    }
    const list: VersionInfo[] = [];
    let v = cat.version;
    const have = apps.find((x) => x.id === id);
    for (let i = 0; i < 6; i++) {
      list.push({
        version: v,
        date: iso((i * 24 + 9) * DAY).slice(0, 10),
        notes: i < 3 ? NOTES[i % NOTES.length] : [],
        notesUrl: null,
        size: cat.size ? Math.round(cat.size * (1 - i * 0.012)) : null,
        installed: !!have && have.version === v,
        latest: i === 0,
        prerelease: false,
      });
      v = prevVersion(v);
    }
    return list;
  },

  pause_job({ jobId }) {
    const j = jobs.find((x) => x.id === jobId);
    // like the backend: only a download (or a queued job) can be paused
    if (j && (j.state === "downloading" || j.state === "waiting")) {
      hidden.set(j.id, { resume: j.state });
      j.state = "paused";
      j.speed = 0;
      j.eta = null;
      j.step = `Paused at ${(j.downloaded / GB).toFixed(1)} of ${(j.total / GB).toFixed(1)} GB`;
      const a = findApp(j.appId);
      if (a) syncApp(a, j);
      emit("jobs", jobs);
      emit("apps", apps);
    }
  },
  resume_job({ jobId }) {
    const j = jobs.find((x) => x.id === jobId);
    if (j && j.state === "paused") {
      j.state = hidden.get(j.id)?.resume ?? "downloading";
      if (j.state === "waiting") j.step = "Waiting";
      const a = findApp(j.appId);
      if (a) syncApp(a, j);
      emit("jobs", jobs);
      emit("apps", apps);
    }
  },
  cancel_job({ jobId }) {
    const j = jobs.find((x) => x.id === jobId);
    // like the backend: an installer that is running cannot be stopped safely
    if (j && isOpen(j.state) && j.state !== "installing" && j.state !== "verifying") {
      j.state = "canceled";
      j.speed = 0;
      j.eta = null;
      j.step = "Canceled";
      const a = findApp(j.appId);
      if (a) {
        syncApp(a, j);
        if (j.kind === "install" && a.status !== "current" && a.version === j.toVersion && !a.updatedAt) apps = apps.filter((x) => x.id !== a.id);
      }
      emit("jobs", jobs);
      emit("apps", apps);
    }
  },
  retry_job({ jobId }) {
    const j = jobs.find((x) => x.id === jobId);
    const a = j && findApp(j.appId);
    if (!j || !a) return;
    jobs = jobs.filter((x) => x.id !== jobId);
    enqueue(a, j.kind, j.toVersion, j.total);
    emit("jobs", jobs);
    emit("apps", apps);
  },
  reorder_jobs({ jobIds }) {
    const waiting = jobs.filter((j) => j.state === "waiting");
    const ordered = jobIds.map((id) => waiting.find((w) => w.id === id)).filter((x): x is Job => !!x);
    const rest = jobs.filter((j) => j.state !== "waiting");
    jobs = [...rest, ...ordered];
    emit("jobs", jobs);
  },
  pause_all() {
    for (const j of [...jobs]) if (isOpen(j.state) && j.state !== "paused" && j.state !== "blocked") handlers.pause_job({ jobId: j.id });
  },
  resume_all() {
    for (const j of [...jobs]) if (j.state === "paused") handlers.resume_job({ jobId: j.id });
  },
  clear_finished_jobs() {
    jobs = jobs.filter((j) => isOpen(j.state));
    for (const a of apps) if (a.status === "failed" && !a.jobId) a.status = baseStatus(a);
    emit("jobs", jobs);
    emit("apps", apps);
  },
  close_and_continue({ jobId }) {
    const j = jobs.find((x) => x.id === jobId);
    if (!j || j.state !== "blocked") return;
    const a = findApp(j.appId);
    if (a) a.running = false;
    j.blockedBy = [];
    j.state = "installing";
    j.step = "Installing, step 2 of 4";
    j.progress = Math.max(j.progress, 0.81);
    if (a) syncApp(a, j);
    emit("jobs", jobs);
    emit("apps", apps);
  },

  get_activity: () => clone(activity),
  export_activity: () => "C:\\Users\\Cloud\\Downloads\\updater-activity.txt",
  open_log: () => {},

  get_services() {
    for (const s of services) {
      if (s.running) {
        s.cpu = Math.max(0, +(s.cpu + (Math.random() - 0.5) * 0.2).toFixed(1));
        s.memory = Math.max(8 * MB, s.memory + (Math.random() - 0.5) * 2 * MB);
      }
    }
    return clone(services);
  },
  stop_service({ name }) {
    const s = services.find((x) => x.name === name);
    if (s) {
      s.running = false;
      s.cpu = 0;
      s.memory = 0;
    }
  },
  start_service({ name }) {
    const s = services.find((x) => x.name === name);
    if (s) {
      s.running = true;
      s.memory = 60 * MB;
      if (s.required) {
        status.ccRunning = services.filter((x) => x.required).every((x) => x.running);
        emit("status", status);
      }
    }
  },
  stop_safe_services() {
    for (const s of services) if (s.safeToStop && s.running) handlers.stop_service({ name: s.name });
  },
  open_creative_cloud() {
    for (const s of services) if (s.required) handlers.start_service({ name: s.name });
    status.ccRunning = true;
    emit("status", status);
    if (!status.signedIn) {
      setTimeout(() => {
        status.signedIn = true;
        emit("status", status);
        emit("toast", { kind: "ok", text: "Signed in through Creative Cloud" });
      }, 2200);
    }
  },

  set_settings({ patch }) {
    settings = { ...settings, ...patch, sources: { ...settings.sources, ...(patch.sources ?? {}) } };
    emit("settings", settings);
    return clone(settings);
  },
  pick_folder: ({ current }) => (current.startsWith("D:") ? "E:\\Blender" : "D:\\Blender"),
  clear_cache() {
    const n = status.cacheSize;
    status.cacheSize = 0;
    status.disks[0].free += n;
    emit("status", status);
    return n;
  },
  show_main: () => {
    location.href = "/";
  },
  quit_app: () => {},
  test_notification() {
    emit("toast", { kind: "info", text: "Test notification sent. Look in the Windows notification area." });
  },
  install_self_update() {
    status.selfUpdate = null;
    emit("status", status);
    emit("toast", { kind: "info", text: "The Updater 3D is restarting to finish the update" });
  },
};

export function handle<K extends keyof Commands>(cmd: K, args: Commands[K]["args"]): Promise<Commands[K]["ret"]> {
  const fn = handlers[cmd] as (a: Commands[K]["args"]) => Commands[K]["ret"] | Promise<Commands[K]["ret"]>;
  return Promise.resolve().then(() => fn(args));
}

// Failed scenario: a toast shortly after load
if (MODE === "failed") setTimeout(() => emit("toast", { kind: "err", text: "Substance 3D Painter update failed. Error 183, files in use.", appId: "adobe:SBSTP" }), 1200);

// Installing scenario: the installer step is running, so Pause and Cancel are disabled
if (MODE === "installing") {
  handlers.update_all({});
  handlers.install_app({ id: "adobe:ILST", version: null });
  handlers.install_app({ id: "craft:vectorcraft", version: null });
  for (const j of jobs) {
    if (j.appId === "craft:figcraft") continue; // keeps downloading, so Pause and Cancel stay usable there
    j.state = "installing";
    j.downloaded = j.total;
    j.progress = 0.86;
    j.step = "Installing, step 2 of 4";
    j.eta = 40;
    j.startedAt = new Date().toISOString();
    held.add(j.id);
  }
  emit("jobs", jobs);
  emit("apps", apps);
}
