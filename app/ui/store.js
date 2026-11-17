// What the library pages share: the search and its results, the page shown, the sound on the
// card, the sound previewing.

/** @type {import('./search.js').LibState} */
export const state = {
  query: '', category: null, folder: null, under: null, subs: [], project: null, tag: null, minRating: null,
  sort: 'score', items: [], selected: -1, projects: [], indexed: -1,
  currentView: 'sounds',
  /** The sound shown on the card. */
  cardItem: null,
  /** Every tag with how many sounds have it. */
  allTags: [],
  /** The sound previewing in the Library. */
  playingItem: null,
  libLoop: false,
};
