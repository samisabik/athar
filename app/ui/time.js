// Durations as the interface writes them.
/** @param {number | null | undefined} sec */
export function fmtTime(sec, precise = false) {
  if (sec == null || !isFinite(sec)) return '';
  if (sec < 60) return precise ? sec.toFixed(3) + 's' : sec < 10 ? sec.toFixed(2) + 's' : sec.toFixed(1) + 's';
  const m = Math.floor(sec / 60), s = sec - m * 60;
  return `${m}:${(precise ? s.toFixed(3) : s.toFixed(0)).padStart(precise ? 6 : 2, '0')}`;
}
