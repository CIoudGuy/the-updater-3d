// Turns a backend call that can reject into a plain result, so a screen can show an error with Retry instead of loading forever.

export type Loaded<T> = { ok: true; value: T } | { ok: false; error: string };

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return "Something went wrong";
}

export async function tryLoad<T>(fn: () => Promise<T>): Promise<Loaded<T>> {
  try {
    return { ok: true, value: await fn() };
  } catch (e) {
    return { ok: false, error: errorText(e) };
  }
}
