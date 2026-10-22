// Hand-drawn SVG shapes, copied from the design export.

/** viewBox 0 0 400 24, stretched: underlines, progress strokes, nav marker */
export const BRUSH =
  "M3 9.5C40 6.8 92 7.9 140 6.6s104-.4 150 .9 74 1.3 104 2.6c3 .3 4 2.4 3.2 4.6-.6 1.8-2.4 2.8-5 3-34 2-82 1.8-128 2.2s-96 .8-140 1.4S30 21 8 19.6C4.4 19.4 1.6 17.4 1.3 14.6 1 12.2 1.4 9.7 3 9.5z";
/** extra pencil strokes of the headline underline (viewBox 0 0 400 24) */
export const BRUSH_LINES = "M18 8.6c70-1.8 160-2.4 250-1.2M40 18.8c90 .6 200-.2 330-1.8";

/** viewBox 0 0 120 36: buttons */
export const BTN =
  "M5 3.2C30 1.8 80 1.6 114 2.8c3 .1 4.8 2.4 5 5.6.4 6.4.4 13.4 0 19.6-.2 3.4-2 5.4-5.2 5.6-30 1.6-78 1.8-108 .4C2.6 33.8 1 31.8.8 28.6.4 22 .4 14.4.9 8 1.1 5.2 2.6 3.4 5 3.2z";
export const BTN_HI = "M10 8c30-1.2 70-1.4 100-.6";
export const BTN_BRISTLE = "M10 9c30-1.2 70-1.4 100-.6M14 28c30 .8 64 .6 92-.4";
export const BTN_DANGER = "M12 29c30 .8 64 .6 96-.4";

/** viewBox 0 0 200 200, stretched: tiles, menus, dialogs, toasts */
export const BLOB =
  "M8 4C50 2 140 1.6 192 3.4c4 .2 5.6 2.6 5.8 6.6 1 50 1 130 .2 180-.1 4-2.2 6-6.2 6.2-50 2-140 2.4-184 .6C3.6 196.6 2 194.4 1.8 190.4 1 140 .8 60 1.6 10 1.7 6.4 3.6 4.2 8 4z";
export const BLOB_LINES = "M16 14c50-1.4 120-1.6 170-.4M20 186c60 1 110 .8 160-.6";

/** viewBox 0 0 200 36: nav swatch, menu highlight, wide chips */
export const SWATCH =
  "M5 5.6C30 3.2 72 4.4 112 3.2s68 .7 82 2.3c3.6.4 5.4 3 5.3 7-.2 4.8 0 9.8-.4 13.8-.3 4.1-2.6 6.3-6.5 6.6-24 1.9-62 .9-100 1.6S24 35.4 7.2 33.2C3.6 32.7 1.6 30.5 1.4 26.8 1.1 21.2 1 15.4 1.5 10.6 1.8 7.6 2.8 5.8 5 5.6z";

/** viewBox 0 0 48 48: app mark */
export const MARK =
  "M9 3.5C17 2.6 31 2.4 39.5 3.6c3.4.5 5 2.8 5.2 6.4.5 9 .6 19.5-.2 28.4-.3 3.5-2.4 5.6-5.8 6-8.6.9-21.6 1.1-30.2.2C5 44.2 3.3 42 3 38.4 2.3 29.5 2.4 18.6 3.1 9.9 3.4 6.2 5.4 3.9 9 3.5z";
export const MARK_LINES = "M10 9c8-1 19-1.2 28-.4M9 40c10 .8 22 .9 31 .2";
export const MARK_LINES_BIG = "M10 9c8-1 19-1.2 28-.4M9 40c10 .8 22 .9 31 .2M7 16c-.4 6-.4 12 0 18";

/** viewBox 0 0 20 20: status dab */
export const DAB =
  "M10.4 1.8c3.6.2 6.9 2.4 7.6 5.9.8 3.8-.9 7.6-4.3 9.3-3.2 1.6-7.4 1-9.6-1.8C1.9 12.4 1.5 8.3 3.6 5.3 5.1 3.1 7.6 1.6 10.4 1.8z";

/** viewBox 0 0 300 300: decorative blobs behind page headers */
export const SPLASH = [
  "M150 18c52-4 104 26 124 74 22 52 6 118-40 150-42 30-110 38-156 8C30 220 8 168 18 118 28 64 92 22 150 18z",
  "M60 40c40-22 120-30 170-6 40 20 64 70 52 116-10 40-40 58-30 92 6 22-28 40-70 36-50-4-90-28-120-62C30 180 6 140 10 104 14 76 34 54 60 40z",
  "M90 30c50-14 120 0 150 40 26 36 20 84-8 116-30 34-80 50-124 40C62 216 26 186 20 146 14 100 42 44 90 30z",
];
export const SPLASH_LINES = "M120 40c40-6 86 8 110 40M104 58c50-6 96 10 120 44M150 110c-30 10-70 40-90 80M200 180c20 10 36 30 40 56";

/** Seamless wave edge for the tile wash, viewBox 0 0 400 100 (two periods) */
export const WAVE_EDGE = "M0 4C25 1 75 7 100 4C125 1 175 7 200 4C225 1 275 7 300 4C325 1 375 7 400 4";
export const WAVE_FILL = WAVE_EDGE + "V100H0z";
