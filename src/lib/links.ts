// Where a click on an app name goes.
import { appHref, getHref } from "./router.svelte";
import { store } from "./store.svelte";
import type { Job } from "./types";

/** A job's app: its page in Installed, or its Get apps page while it is still being installed.
 * Nothing for an app that was removed. */
export function jobHref(j: Job): string | null {
  if (store.app(j.appId)) return appHref(j.appId);
  if (j.kind === "install") {
    const done = store.apps.find((a) => a.id.startsWith(j.appId + ":"));
    return done ? appHref(done.id) : getHref(j.appId);
  }
  return null;
}
