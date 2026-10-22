// Formatting helpers. No dashes in output, tabular numbers, short and human.

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** 2.9 GB, 312 MB, 18 KB */
export function bytes(n: number | null | undefined): string {
  if (n == null || !isFinite(n)) return "";
  if (n < 1000) return `${Math.round(n)} B`;
  let v = n;
  let u = 0;
  while (v >= 1000 && u < UNITS.length - 1) {
    v /= 1000;
    u++;
  }
  const digits = u >= 3 ? (v >= 100 ? 0 : 1) : 0;
  return `${v.toFixed(digits)} ${UNITS[u]}`;
}

/** Value and unit separately, for big numbers: ["2.7", "GB"] */
export function bytesParts(n: number, unit?: string): [string, string] {
  let u = unit ? UNITS.indexOf(unit) : 0;
  let v = n;
  if (unit) {
    v = n / Math.pow(1000, u);
  } else {
    while (v >= 1000 && u < UNITS.length - 1) {
      v /= 1000;
      u++;
    }
  }
  const digits = u >= 3 ? (v >= 100 ? 0 : 1) : v >= 100 ? 0 : v >= 10 ? 0 : 1;
  return [v.toFixed(u < 2 ? 0 : digits), UNITS[u]];
}

/** Unit that suits a total, so "2.7 of 5.4 GB" share one unit */
export function unitFor(n: number): string {
  let v = n;
  let u = 0;
  while (v >= 1000 && u < UNITS.length - 1) {
    v /= 1000;
    u++;
  }
  return UNITS[u];
}

/** Number and unit of a speed, for places that show them apart: ["38", "MB/s"], ["380", "KB/s"] */
export function speedParts(bps: number): [string, string] {
  if (!bps || bps < 1) return ["0", "MB/s"];
  const mb = bps / 1e6;
  if (mb >= 10) return [String(Math.round(mb)), "MB/s"];
  if (mb >= 1) return [mb.toFixed(1), "MB/s"];
  return [String(Math.max(1, Math.round(bps / 1e3))), "KB/s"];
}

/** 38 MB/s */
export function speed(bps: number): string {
  const [n, u] = speedParts(bps);
  return `${n} ${u}`;
}

/** 1 min 15 s, 20 s, 1 h 5 min */
export function duration(sec: number | null | undefined): string {
  if (sec == null || !isFinite(sec) || sec < 0) return "";
  const s = Math.round(sec);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) {
    const r = s % 60;
    return r ? `${m} min ${r} s` : `${m} min`;
  }
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return rm ? `${h} h ${rm} min` : `${h} h`;
}

/** About 4 min, About 20 s, About 1 h 10 min */
export function approx(sec: number | null | undefined): string {
  if (sec == null || !isFinite(sec) || sec < 0) return "";
  if (sec < 45) return `About ${Math.max(5, Math.round(sec / 5) * 5)} s`;
  const m = Math.round(sec / 60);
  if (m < 60) return `About ${Math.max(1, m)} min`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return rm ? `About ${h} h ${rm} min` : `About ${h} h`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTHS_LONG = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

function sameDay(a: Date, b: Date) {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

/** 3 Oct, Today, 3 Oct 2025 */
export function dateShort(iso: string | null | undefined, now = new Date()): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(+d)) return "";
  if (sameDay(d, now)) return "Today";
  const base = `${d.getDate()} ${MONTHS[d.getMonth()]}`;
  return d.getFullYear() === now.getFullYear() ? base : `${base} ${d.getFullYear()}`;
}

/** 3 Oct 2026 */
export function dateLong(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(+d)) return "";
  return `${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()}`;
}

/** Activity day label: Today, Yesterday, 15 October */
export function dayLabel(iso: string, now = new Date()): string {
  const d = new Date(iso);
  if (sameDay(d, now)) return "Today";
  const y = new Date(now);
  y.setDate(y.getDate() - 1);
  if (sameDay(d, y)) return "Yesterday";
  const base = `${d.getDate()} ${MONTHS_LONG[d.getMonth()]}`;
  return d.getFullYear() === now.getFullYear() ? base : `${base} ${d.getFullYear()}`;
}

/** 9:42 (24 hour, no leading zero on the hour) */
export function clock(iso: string | Date | null | undefined): string {
  if (!iso) return "";
  const d = typeof iso === "string" ? new Date(iso) : iso;
  if (isNaN(+d)) return "";
  return `${d.getHours()}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** 09:42 with a leading zero, for activity rows */
export function clock2(iso: string): string {
  const d = new Date(iso);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function plural(n: number, one: string) {
  return `${n} ${one}${n === 1 ? "" : "s"}`;
}

/** 3 minutes ago */
export function ago(iso: string, nowMs = Date.now()): string {
  const s = Math.max(0, (nowMs - +new Date(iso)) / 1000);
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${plural(Math.max(1, m), "minute")} ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${plural(h, "hour")} ago`;
  return `${plural(Math.round(h / 24), "day")} ago`;
}

/** Checked at 9:42, 3 minutes ago */
export function checkedLine(iso: string | null, nowMs = Date.now()): string {
  if (!iso) return "Not checked yet";
  const d = new Date(iso);
  const age = nowMs - +d;
  if (age > 36e5 * 20) return `Checked ${dateShort(iso)} at ${clock(iso)}`;
  return `Checked at ${clock(iso)}, ${ago(iso, nowMs)}`;
}

/** Split a long version for display: "26.001.20437" -> ["26.001", ".20437"] */
export function splitVersion(v: string): [string, string] {
  if (v.length <= 7) return [v, ""];
  const parts = v.split(".");
  if (parts.length < 3) return [v, ""];
  return [parts.slice(0, 2).join("."), "." + parts.slice(2).join(".")];
}

/** "5.4.0 Alpha fbe82a4" -> ["5.4.0", "Alpha fbe82a4"]; a plain version stays whole. */
export function preParts(v: string): [string, string] {
  const i = v.indexOf(" ");
  return i > 0 ? [v.slice(0, i), v.slice(i + 1)] : [v, ""];
}

/** "A", "A and B", "A, B and C", "A, B, C and 2 more" */
export function nameList(names: string[], max = 3): string {
  if (names.length === 0) return "";
  if (names.length === 1) return names[0];
  if (names.length <= max) return names.slice(0, -1).join(", ") + " and " + names[names.length - 1];
  const rest = names.length - max;
  return names.slice(0, max).join(", ") + ` and ${rest} more`;
}

export function countOf(n: number, one: string, many = one + "s") {
  return `${n} ${n === 1 ? one : many}`;
}

/** Drive letter of a Windows path: "C:" */
export function driveOf(path: string | null | undefined): string | null {
  if (!path) return null;
  const m = /^([a-zA-Z]):/.exec(path);
  return m ? m[1].toUpperCase() + ":" : null;
}

/** Simple semantic compare of dotted versions (for sorting) */
export function cmpVersion(a: string, b: string): number {
  const pa = a.split(/[.\-]/).map((x) => parseInt(x, 10) || 0);
  const pb = b.split(/[.\-]/).map((x) => parseInt(x, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d) return d;
  }
  return 0;
}

/** Bytes expressed in a unit ("GB"), as a plain number */
export function inUnit(n: number, unit: string): number {
  return n / Math.pow(1000, Math.max(0, UNITS.indexOf(unit)));
}
