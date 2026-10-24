// What an app's mark shows while its icon is missing, on its way, in or broken. Kept apart from
// Mark.svelte so it can be tested: the component only draws what this says.

/** What the picture of an icon is doing: no address yet, being fetched, decoded, or broken. */
export type Load = "none" | "loading" | "loaded" | "failed";

export interface MarkInput {
  /** the id of the app ("other:7zip.7zip", "prog:...", "driver:..."); a job's id for a job */
  id?: string;
  iconUrl: string | null;
  /** the icon search is over and found none */
  iconMiss?: boolean;
}

export interface MarkShow {
  /** the painted brand mark (letters on a colour) of the apps that have one: Adobe, Blender, ... */
  painted: boolean;
  /** the default "no image" tile: every other app until its real picture is in */
  placeholder: boolean;
  /** the soft moving highlight: a picture is on its way (or still being looked for) */
  shimmer: boolean;
  /** the picture is in the page (it fades in once it has decoded) */
  image: boolean;
}

/** Apps with a painted brand mark of their own, which they keep. */
export function isPainted(id?: string): boolean {
  return /^(adobe|craft|blender|tools):/.test(id ?? "");
}

/** Apps whose icon is looked for in the background, so it may still come. */
const LOOKED_FOR = /^(other|prog|msstore):/;

/** What the mark is made of, from what is known of the app's icon and its picture. */
export function markShow(a: MarkInput, load: Load): MarkShow {
  const image = !!a.iconUrl && load !== "failed";
  const loaded = load === "loaded";
  if (isPainted(a.id)) return { painted: !loaded, placeholder: false, shimmer: false, image };
  // a picture that is on its way, or an icon that is still being looked for; never a broken one
  const coming = a.iconUrl ? load === "loading" || load === "none" : LOOKED_FOR.test(a.id ?? "") && !a.iconMiss;
  return { painted: false, placeholder: !loaded, shimmer: coming && !loaded, image };
}
