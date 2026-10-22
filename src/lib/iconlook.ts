// How an app icon looks, for the dark theme.

/**
 * Is a picture mostly dark where it is not clear (a black logo on a clear background)? `rgba` is
 * straight RGBA, as a canvas gives it. A dark tile with a light sign on it is not: the sign reads
 * on its own. The backend tells the same for the icons it saves (icons.rs, `mostly_dark`).
 */
export function mostlyDark(rgba: ArrayLike<number>): boolean {
  let seen = 0;
  let light = 0;
  let lum = 0;
  for (let i = 0; i + 3 < rgba.length; i += 4) {
    if (rgba[i + 3] < 128) continue;
    const l = (0.2126 * rgba[i] + 0.7152 * rgba[i + 1] + 0.0722 * rgba[i + 2]) / 255;
    seen++;
    lum += l;
    if (l > 0.45) light++;
  }
  return seen > 0 && lum / seen < 0.22 && light < seen * 0.06;
}
