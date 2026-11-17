import test from 'node:test';
import assert from 'node:assert/strict';
import { audible } from '../athar/static/arrange.js';

const lane = (o) => ({ mute: false, solo: false, ...o });

test('mute silences a lane and solo silences the others', () => {
  const a = lane({}), b = lane({ mute: true }), c = lane({ solo: true }), d = lane({ solo: true, mute: true });
  assert.deepEqual([a, b].map(l => audible(l, [a, b])), [true, false]);
  assert.deepEqual([a, b, c, d].map(l => audible(l, [a, b, c, d])), [false, false, true, false]);
});
