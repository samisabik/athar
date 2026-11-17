// Cat skins. Each animation is a sheet row and a frame sequence; skins without a left row mirror the right one.
export const CAT_SKINS = {
  // Sukey the Tuxedo Cat (EgotisticalSlug), a Stardew Valley pet sheet: 32 px frames, 4 per row.
  sukey: {
    name: 'Leaflet', src: '/static/cat-sukey.png', px: 32, size: 128, smooth: false, speed: 240,
    anim: {
      right: { row: 1, seq: [0, 1, 2, 3], ms: 75 }, left: { row: 3, seq: [0, 1, 2, 3], ms: 75 },
      sitdown: { row: 4, seq: [0, 1], ms: 140 },
      idle: { row: 4, seq: [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3], ms: 220 },
      groom: { row: 5, seq: [0, 1, 2, 1, 2, 3, 0, 1, 2, 3], ms: 220 },
    },
  },
  // Sheets built from the free packs in cats/: one row each for idle and run.
  lemon: {
    name: 'Lemon', src: '/static/cats/lemon.png', px: 32, size: 96, smooth: false, speed: 300,
    anim: { idle: { row: 0, seq: [0, 1, 2, 3, 4, 5, 6, 7], ms: 260 }, right: { row: 1, seq: [0, 1, 2, 3, 4, 5, 6, 7], ms: 80 } },
  },
  pepper: {
    name: 'Reglisse', src: '/static/cats/pepper.png', px: 16, size: 96, smooth: false, speed: 300,
    anim: { idle: { row: 0, seq: [0, 1, 2, 3], ms: 260 }, right: { row: 1, seq: [0, 1, 2, 3], ms: 80 } },
  },
  biscuit: {
    name: 'Chaussette', src: '/static/cats/biscuit.png', px: 32, size: 96, smooth: false, speed: 300,
    anim: { idle: { row: 0, seq: [0, 1, 2, 3], ms: 260 }, right: { row: 1, seq: [0, 1, 2, 3, 4, 5, 6, 7], ms: 80 } },
  },
};
