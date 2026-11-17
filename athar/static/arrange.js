// The Arranger's limits, and the rule for which lanes sound (the native side plays by the same
// rule; the page uses it to dim the lanes that are silent).
export const MAX_LANES = 16, MAX_HITS = 64, MAX_SECONDS = 10 * 60;
export const MIN_GAIN_DB = -24, MAX_GAIN_DB = 6;

/** @typedef {{ mute?: boolean, solo?: boolean }} Lane */

/** A lane sounds unless it is muted, or another lane is soloed and it is not.
    @param {Lane} lane @param {Lane[]} lanes */
export function audible(lane, lanes) {
  return !lane.mute && (!!lane.solo || !lanes.some(l => l.solo));
}
