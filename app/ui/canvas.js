// What every canvas drawing in the interface starts with: a bitmap at device resolution, the
// theme's colours and the interface font.

const MONO = '"Share Tech Mono", "Martian Mono", ui-monospace, monospace';
/** The interface font at `px` pixels, for a canvas. @param {number} px */
export const monoFont = (px) => `${px}px ${MONO}`;

/**
 * Sizes a canvas's bitmap to its box at device resolution and returns its context, scaled so
 * that drawing is in CSS pixels. Null while the canvas has no box.
 * @param {HTMLCanvasElement} cv
 * @returns {{ g: CanvasRenderingContext2D, W: number, H: number, dpr: number } | null}
 */
export function fitCanvas(cv) {
  const dpr = window.devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight;
  if (!W || !H) return null;
  if (cv.width !== W * dpr || cv.height !== H * dpr) { cv.width = W * dpr; cv.height = H * dpr; }
  const g = /** @type {CanvasRenderingContext2D} */ (cv.getContext('2d'));
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { g, W, H, dpr };
}

/** A reader of the theme's colours by custom property name, as they are right now. @returns {(name: string) => string} */
export function palette() {
  const css = getComputedStyle(document.documentElement);
  return (name) => css.getPropertyValue(name).trim();
}
