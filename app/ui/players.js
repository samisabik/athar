// One thing sounds at a time. Everything that plays (the Library preview, the editor, Discover,
// the Recorder's takes, the Arranger) says how to silence it, and silences the others when it starts.

/** @type {Map<string, () => void>} */
const stops = new Map();

/** @param {string} name @param {() => void} stop Stops this player; does nothing when it is not playing. */
export function registerPlayer(name, stop) { stops.set(name, stop); }

/** Stops every player but `name`. @param {string} name */
export function stopOthers(name) {
  for (const [other, stop] of stops) if (other !== name) stop();
}
