// Keyboard shortcut rules that do not depend on the UI, so they can be tested.

export type ShortcutAction =
  | { do: "retry"; jobIds: string[] }
  | { do: "repair"; appId: string }
  | { do: "open"; appId: string }
  | null;

interface KeyApp {
  id: string;
  status: string;
  canRepair: boolean;
}

/**
 * R and O on the app page or on the tile or row under the pointer.
 * R retries a failed app or repairs one with missing files; O opens a finished app.
 * On Home with failed jobs and no app in focus, R retries all of them.
 */
export function appShortcut(
  key: string,
  o: { route: string; app: KeyApp | undefined; hasJob: boolean; failedJobIds: string[]; allFailedJobIds: string[] },
): ShortcutAction {
  const k = key.toLowerCase();
  const { app } = o;
  if (k === "r") {
    if (app && !o.hasJob) {
      if (app.status === "failed" && o.failedJobIds.length) return { do: "retry", jobIds: o.failedJobIds };
      if (app.status === "missing" && app.canRepair) return { do: "repair", appId: app.id };
    }
    if (o.route === "home" && o.allFailedJobIds.length) return { do: "retry", jobIds: o.allFailedJobIds };
    return null;
  }
  if (k === "o" && app && !o.hasJob && app.status !== "missing" && (o.route === "installed" || o.route === "detail")) {
    return { do: "open", appId: app.id };
  }
  return null;
}

/** A dialog, or the "close the app" prompt for a blocked job that was not dismissed, is on screen: page shortcuts must not run behind it. */
export function modalOpen(o: { dialog: boolean; blockedIds: string[]; dismissed: string[] }): boolean {
  return o.dialog || o.blockedIds.some((id) => !o.dismissed.includes(id));
}

/** The app under the pointer or focus is forgotten when that same app is left. */
export function focusAfterLeave(current: string | null, leaving: string): string | null {
  return current === leaving ? null : current;
}
