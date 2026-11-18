// Unit tests of the interface's pure helpers. Run with `npm test`; no window is needed.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { baseOf, dirOf, stemOf, extOf, plural } from '../ui/names.js';

test('a path splits into its folder and its name', () => {
  assert.equal(baseOf('/a/b/kick.wav'), 'kick.wav');
  assert.equal(dirOf('/a/b/kick.wav'), '/a/b');
  assert.equal(baseOf('kick.wav'), 'kick.wav');
});

test('only the last dot starts the extension', () => {
  assert.equal(stemOf('kick.v2.wav'), 'kick.v2');
  assert.equal(extOf('kick.v2.wav'), '.wav');
  assert.equal(stemOf('Take 2026-11-17 10.32.05.wav'), 'Take 2026-11-17 10.32.05');
  assert.equal(stemOf('noext'), 'noext');
  assert.equal(extOf('noext'), '');
});

// A batch save names its file with the extension, so the native side, which drops whatever
// follows the last dot, keeps the dots of the stem and the suffix after them.
test('a saved name built from a dotted stem keeps the stem whole', () => {
  const name = `${stemOf('kick.v2.wav')}_edit.flac`;
  assert.equal(stemOf(name), 'kick.v2_edit');
});

test('counts take their plural', () => {
  assert.equal(plural(1, 'sound'), '1 sound');
  assert.equal(plural(0, 'sound'), '0 sounds');
  assert.equal(plural(3, 'clip'), '3 clips');
});
