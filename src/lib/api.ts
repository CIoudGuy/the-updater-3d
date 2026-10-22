// Backend bridge. Inside Tauri it uses invoke/listen; in a normal browser it
// falls back to the in-memory mock (src/lib/mock.ts) so the UI runs at `npm run dev`.
import type { Commands } from "./commands";
import type { EventMap } from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type MockModule = typeof import("./mock");
let mockPromise: Promise<MockModule> | null = null;
function mock(): Promise<MockModule> {
  return (mockPromise ??= import("./mock"));
}

export async function call<K extends keyof Commands>(cmd: K, args?: Commands[K]["args"]): Promise<Commands[K]["ret"]> {
  if (isTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return (await invoke(cmd as string, (args ?? {}) as Record<string, unknown>)) as Commands[K]["ret"];
  }
  const m = await mock();
  return (await m.handle(cmd, (args ?? {}) as never)) as Commands[K]["ret"];
}

/** Fire and forget; failures show up as a toast through the store. */
export function send<K extends keyof Commands>(cmd: K, args?: Commands[K]["args"]): Promise<Commands[K]["ret"] | undefined> {
  return call(cmd, args).catch((e) => {
    console.error(cmd, e);
    onError?.(String(e?.message ?? e));
    return undefined;
  });
}
let onError: ((m: string) => void) | null = null;
export function setErrorHandler(fn: (m: string) => void) {
  onError = fn;
}

export async function on<K extends keyof EventMap>(event: K, cb: (payload: EventMap[K]) => void): Promise<() => void> {
  if (isTauri) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<EventMap[K]>(event, (e) => cb(e.payload));
    return un;
  }
  const m = await mock();
  return m.listen(event, cb);
}

// Window controls (no-ops in the browser)
async function win() {
  if (!isTauri) return null;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}
export const windowApi = {
  async minimize() {
    (await win())?.minimize();
  },
  async toggleMaximize() {
    (await win())?.toggleMaximize();
  },
  async close() {
    (await win())?.close();
  },
  async isMaximized(): Promise<boolean> {
    return (await (await win())?.isMaximized()) ?? false;
  },
  async onResized(cb: () => void): Promise<() => void> {
    const w = await win();
    if (!w) return () => {};
    return await w.onResized(cb);
  },
};
