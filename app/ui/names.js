// File names and counts, as the interface writes them.

/** The last part of a path: the file's or folder's own name. @param {string} path */
export const baseOf = (path) => path.slice(path.lastIndexOf('/') + 1);
/** The folder a path is in. @param {string} path */
export const dirOf = (path) => path.slice(0, path.lastIndexOf('/'));
/** A file name without its extension. @param {string} name */
export const stemOf = (name) => name.replace(/\.[^.]+$/, '');
/** A file name's extension with its dot, or '' when it has none. @param {string} name */
export const extOf = (name) => (name.match(/\.[^.]+$/) || [''])[0];
/** A count with its noun: "1 sound", "3 sounds". @param {number} n @param {string} word */
export const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
