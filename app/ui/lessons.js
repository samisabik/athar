// Short explanations the cat mixes in with its tips. SECTION_LESSONS are keyed by the editor
// section that is open (an effect tab or a tool name from tools.js); PAGE_LESSONS by the page.
// Each card stands on its own, since they are read one at a time among other tips.

export const SECTION_LESSONS = {
  eq: {
    title: 'Parametric EQ',
    cards: [
      'An EQ makes some frequencies louder or quieter. Low frequencies are on the left of the graph, high ones on the right.',
      'Each dot is a band. Drag it sideways to choose the frequency, up to boost, down to cut. Scroll on it to make it wider or narrower: that width is called Q.',
      'A bell changes one area. A shelf changes everything above or below a point. A low cut removes everything below a point, a high cut everything above. A notch removes one narrow spot.',
      'Cutting what you dislike usually sounds more natural than boosting what you like. Flick the switch to Original to check you improved it.',
    ],
  },
  comp: {
    title: 'Compressor',
    cards: [
      'A compressor turns the loud parts down, so loud and quiet parts end up closer together. Then Makeup turns the whole sound up, and it feels fuller.',
      'Threshold is the level where it starts working. Everything louder than the threshold is turned down. Lower it and more of the sound is compressed.',
      'Ratio is how hard it turns down. At 4:1, a sound 4 dB over the threshold comes out 1 dB over. Higher is squashed harder.',
      'Attack is how fast it reacts once the sound passes the threshold. A slow attack lets the first snap of a drum through before it clamps. A fast one flattens the snap.',
      'Release is how fast it lets go once the sound drops back under the threshold. Short sounds lively and can pump. Long sounds smooth.',
      'Knee softens the corner at the threshold, so compression fades in gradually. Makeup adds the lost level back. The button next to it sets it for you.',
      'The graph shows level in against level out. The bend is the threshold, and the flatter the line after it, the higher the ratio.',
    ],
  },
  max: {
    title: 'Maximizer',
    cards: [
      'A maximizer makes a sound as loud as it can be. It turns the sound up, and a limiter holds the peaks under a ceiling so they do not clip.',
      'Amplify is how much it turns up. The more you push, the more the peaks are held back, and the denser the sound gets.',
      'Ceiling is the highest level any peak may reach. Just under 0 dB is the usual choice.',
      'Release is how fast the limiter lets go after a peak. Too short can distort bass. Too long ducks the sound after every hit.',
    ],
  },
  sat: {
    title: 'Saturation',
    cards: [
      'Saturation rounds off the peaks of a sound, the way tape or an overloaded amp does. That adds harmonics, which make it warmer and easier to hear on small speakers.',
      'Drive pushes the sound harder into the rounding. More drive, more grit.',
      'Ceiling is the level the rounded peaks stay under. Mix blends the saturated sound with the clean one.',
    ],
  },
  shape: {
    title: 'Shape',
    cards: [
      'Shape changes the start and the end of each hit separately. It follows how fast the level moves, so it works on loud and quiet hits alike.',
      'Attack up gives each hit more punch. Attack down softens it.',
      'Tail up lets the ring after a hit last longer. Tail down dries it up, which tightens a boomy drum or a roomy recording.',
    ],
  },
  lofi: {
    title: 'Lo-fi',
    cards: [
      'Lo-fi makes a sound rougher, like an old sampler. It does two things: fewer bits and a lower sample rate.',
      'Bit depth is how finely the level of each sample is stored. Fewer bits add a grainy noise, clearest on quiet tails.',
      'Sample rate is how many samples there are per second. A lower rate dulls the top end and adds a metallic ring.',
    ],
  },
  nr: {
    title: 'Noise reduction',
    cards: [
      'Noise reduction turns down steady background noise such as hiss, hum or air conditioning. It works on each frequency separately.',
      'It needs to know what the noise is. Select a moment with only noise and press the capture button. Without that, it estimates the noise by itself.',
      'Reduction is how much the noise is turned down. Too much makes the sound watery, so use the least that works.',
      'Sensitivity sets how far above the noise a sound must be to count as wanted. In the graph the fill is the noise and the line is that limit.',
    ],
  },
  fade: {
    title: 'Fade',
    cards: [
      'A fade in raises the level from silence at the start. A fade out lowers it to silence at the end. Either keeps a sound from starting or stopping with a click.',
      'Each ruler is how long that fade lasts. A few milliseconds only removes a click. A second or more is a fade you can hear. Leave one at 0 to fade only the other end.',
      'Select part of the waveform and the fades sit at the two ends of that part. You can also drag the end of each curve on the waveform.',
    ],
  },
  normalize: {
    title: 'Normalize peak',
    cards: [
      'Normalizing turns the whole sound up or down by one amount, so its highest peak lands on the level you set. The balance inside the sound stays the same.',
      'Peak is that level. 0 dB is the most a file can hold, so a little under it leaves room.',
      'Peak level is not loudness. Two sounds with the same peak can sound very different in volume. For matching volume, use Normalize loudness.',
    ],
  },
  loudnorm: {
    title: 'Normalize loudness',
    cards: [
      'This sets how loud the sound is to the ear, measured in LUFS, a standard loudness unit. Sounds set to the same LUFS play back at a similar volume.',
      'Loudness is the target. Less negative is louder: -14 is louder than -23.',
      'A loud target can push the peaks over full scale. The app warns you when it does, and the Maximizer can then hold the peaks down.',
    ],
  },
  gain: {
    title: 'Gain',
    cards: [
      'Gain turns the sound up or down by a fixed amount. Nothing else about it changes.',
      '6 dB up is about twice the amplitude, 6 dB down about half. If the peak chip at the bottom says it clips, turn it down.',
    ],
  },
  speed: {
    title: 'Speed',
    cards: [
      'Speed plays the sound faster or slower, like a record or a tape. Pitch and length change together.',
      '50% is twice as long and an octave lower. 200% is half as long and an octave higher.',
      'To change only the length or only the pitch, use Stretch and pitch.',
    ],
  },
  stretch: {
    title: 'Stretch and pitch',
    cards: [
      'Stretch changes how long a sound lasts without changing its pitch, and its pitch without changing how long it lasts.',
      'Length is the new duration in percent. Pitch moves the sound up or down in semitones: 12 is one octave.',
      'Large changes smear the sound. That can be a flaw or the effect you want.',
    ],
  },
  tune: {
    title: 'Tune to note',
    cards: [
      'Tune reads the pitch of the sound and shifts it onto a musical note, so it plays in tune with other things.',
      'The Note ruler steps through nearby notes. Snap to limits it to the notes of a key and scale.',
      'The shift works like Speed: going up makes the sound shorter, going down makes it longer.',
    ],
  },
  loopxfade: {
    title: 'Make it loop',
    cards: [
      'A loop clicks or jumps when its end does not match its start. This blends the end of the selection into its start, so it repeats smoothly.',
      'Crossfade is how long that blend lasts. Longer hides a bigger mismatch and shortens the loop by the same amount.',
      'Select the stretch you want to repeat first, turn Loop on, and adjust the crossfade while it plays.',
    ],
  },
  declick: {
    title: 'Remove clicks',
    cards: [
      'This finds short clicks and pops, such as vinyl crackle or a bumped cable, and redraws the wave across each one.',
      'Sensitivity is how readily something counts as a click. Higher catches more, and may start to dull real hits.',
    ],
  },
  trimsilence: {
    title: 'Trim silent ends',
    cards: [
      'This removes the quiet before the sound starts and after it ends, so a sample plays the moment you trigger it.',
      'Silence below is the level under which audio counts as silence. It starts at the bottom, where nothing is cut. Raise it and the waveform covers the parts that would go.',
    ],
  },
  autoslice: {
    title: 'Slice at silences',
    cards: [
      'This cuts a recording into regions wherever it goes quiet. Each region can then be saved as its own file from the File menu.',
      'Silence below is the level that counts as quiet. Slice gap is how long the quiet must last before a new region starts.',
    ],
  },
  transients: {
    title: 'Slice at hits',
    cards: [
      'This puts a region on every hit, such as each drum in a loop. Each region can then be saved as its own file from the File menu.',
      'Sensitivity is how readily something counts as a hit. Raise it if hits are missed, lower it if one hit is cut in two.',
    ],
  },
  gridslice: {
    title: 'Slice on a tempo grid',
    cards: [
      'This cuts the sound into equal pieces that follow a tempo. It suits loops recorded to a steady beat.',
      'Tempo is the speed of the loop in beats per minute. Slices per beat sets how fine the cuts are: 1 cuts on every beat, 4 on every sixteenth note.',
    ],
  },
};

export const PAGE_LESSONS = {
  sounds: {
    title: 'Library and projects',
    cards: [
      'The Library is every sound in the folders you gave Athar. It only reads those files. It never changes them.',
      'A project is a folder of its own. Adding a sound to a project copies the file into that folder, and the original stays in the Library.',
      'Each sound says where it lives: Library, or the name of its project. A copy or an edit also says which sound it was made from.',
      'Library folders, by the search box, lists the folders the Library reads. Add folder gives it another one.',
    ],
  },
  folders: {
    title: 'Library folders',
    cards: [
      'These are the folders the Library reads. Athar analyses each file once, then watches the folder for new ones.',
      'Removing a folder here only takes it out of the Library. The files stay on disk.',
    ],
  },
  projects: {
    title: 'Projects',
    cards: [
      'A project is what you are making: a set of sounds for a track or for the sampler. It has its own folder on disk.',
      'Adding a sound copies it into the project folder. You can edit or delete the copy without touching the original in the Library.',
      'Edits saved from the editor into a project land in the same folder, and Export converts the whole project for the SP-404MKII.',
    ],
  },
  editor: {
    title: 'How the editor works',
    cards: [
      'The editor works on a copy in memory. The file on disk is never changed. Save writes a new file, and asks which project it goes into.',
      'Open an effect or a tool and move a control: you hear the result straight away and the waveform redraws. Turn Loop on and it keeps playing while you adjust.',
      'What you hear in an open section is a preview. Apply puts it in the sound, Discard drops it.',
      'The Original and Edited switch compares the two. The controls stay where they are, and B flicks it from the keyboard.',
      'Open an effect or a tool and click me: I explain what it does and what each control is for.',
    ],
  },
  discover: {
    title: 'Discover',
    cards: [
      'Discover searches free sound archives online. A sound you add is downloaded into the project you choose.',
      'Its licence and source link are written into the note, so you know later where it came from and how you may use it.',
    ],
  },
  recorder: {
    title: 'Recorder',
    cards: [
      'A take stays in memory until you save it. Saving writes a file into the project or Library folder you choose.',
      'Open a take in the editor to trim it first, then save from there.',
    ],
  },
  arranger: {
    title: 'Arranger',
    cards: [
      'The Arranger plays several sounds together. Lanes are tracks you name, such as Drums or Vocal, and each holds clips. Its browser on the left finds sounds; drag one onto a lane to place it. The sounds themselves are not changed.',
      'Save mix writes what you hear as one new file, into the project or Library folder you choose.',
    ],
  },
  map: {
    title: 'Map',
    cards: [
      'The map lays out the whole Library by how sounds sound. Sounds that are alike sit close together.',
      'It is another way to browse the same files as the Library page.',
    ],
  },
};
