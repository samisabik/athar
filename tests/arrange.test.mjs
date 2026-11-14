import test from 'node:test';
import assert from 'node:assert/strict';
import { snapTime, contentEnd, passLength, audible, hitsInPass, mixdown, fitPeak, renderMix } from '../athar/static/arrange.js';

const lane = (o) => ({ duration: 1, hits: [0], mute: false, solo: false, ...o });
const ones = (n, v = 1) => new Float32Array(n).fill(v);
const near = (a, b, eps = 1e-6) => assert.ok(Math.abs(a - b) < eps, `${a} is not ${b}`);

test('sounds snap to sixteenth notes', () => {
  near(snapTime(0.13, 120), 0.125);
  near(snapTime(0.19, 120), 0.25);
  assert.equal(snapTime(-1, 120), 0);
});

test('a pass ends with the last sound, or on a whole bar when the grid is on', () => {
  const lanes = [lane({ hits: [0, 1.5], duration: 0.4 }), lane({ hits: [0.2], duration: 0.5 })];
  near(contentEnd(lanes), 1.9);
  near(passLength({ lanes, grid: false, bpm: 120, bars: 0 }), 1.9);
  near(passLength({ lanes, grid: true, bpm: 120, bars: 0 }), 2);
  near(passLength({ lanes: [lane({ hits: [1.5], duration: 0.5 })], grid: true, bpm: 120, bars: 0 }), 2);
  near(passLength({ lanes, grid: true, bpm: 120, bars: 4 }), 8);
  near(passLength({ lanes: [], grid: true, bpm: 120, bars: 0 }), 2);
  assert.equal(passLength({ lanes: [], grid: false, bpm: 120, bars: 0 }), 0);
});

test('a chosen number of bars leaves out the sounds that start after it', () => {
  assert.deepEqual(hitsInPass(lane({ hits: [0, 1.5, 2, 3] }), 2), [0, 1.5]);
});

test('mute silences a lane and solo silences the others', () => {
  const a = lane({}), b = lane({ mute: true }), c = lane({ solo: true }), d = lane({ solo: true, mute: true });
  assert.deepEqual([a, b].map(l => audible(l, [a, b])), [true, false]);
  assert.deepEqual([a, b, c, d].map(l => audible(l, [a, b, c, d])), [false, false, true, false]);
});

test('the mix adds each sound at its place and level', () => {
  const [m] = mixdown([
    { chs: [ones(4)], sr: 10, gain: 0.5, hits: [0, 0.6] },
    { chs: [ones(2, -1)], sr: 10, gain: 1, hits: [0.2] },
  ], 10, 1);
  assert.deepEqual([...m], [0.5, 0.5, -0.5, -0.5, 0, 0, 0.5, 0.5, 0.5, 0.5]);
});

test('a mono sound goes to both sides of a stereo mix', () => {
  const [l, r] = mixdown([{ chs: [ones(2)], sr: 10, gain: 1, hits: [0] }, { chs: [ones(2, 0.25), ones(2, 0.5)], sr: 10, gain: 1, hits: [0] }], 10, 0.2);
  assert.deepEqual([...l], [1.25, 1.25]);
  assert.deepEqual([...r], [1.5, 1.5]);
});

test('a sound at another sample rate keeps its length in seconds', () => {
  const [m] = mixdown([{ chs: [ones(24000)], sr: 24000, gain: 1, hits: [0.5] }], 48000, 2);
  assert.equal(m.length, 96000);
  assert.equal(m[12000], 0);
  near(m[48000], 1, 0.01);
  assert.equal(m[80000], 0);
});

test('a mix over full scale is turned down, and one under it is left alone', () => {
  const loud = [Float32Array.of(0.5, -2, 1)], fine = [Float32Array.of(0.5, -1)];
  near(fitPeak(loud), 20 * Math.log10(2));
  near(Math.max(...loud[0].map(Math.abs)), Math.pow(10, -0.1 / 20));
  assert.equal(fitPeak(fine), 0);
  assert.deepEqual([...fine[0]], [0.5, -1]);
});

test('a saved mix has the asked rate, channels and length, and a cut end fades out', () => {
  const lanes = [{ chs: [ones(48000, 0.5), ones(48000, 0.5)], sr: 48000, gain: 1, hits: [0] }];
  const { wav, over } = renderMix({ lanes, sr: 48000, seconds: 0.5, rate: 24000, bits: 32, mono: true, fadeEnd: true });
  const v = new DataView(wav);
  assert.equal(over, 0);
  assert.equal(v.getUint16(22, true), 1);
  assert.equal(v.getUint32(24, true), 24000);
  const data = new Float32Array(wav, wav.byteLength - 12000 * 4);
  near(data[6000], 0.5, 0.01);
  assert.ok(Math.abs(data[11999]) < 0.05, `the last sample is ${data[11999]}`);
});

test('a fade out over a fade in keeps the level even where two clips meet', () => {
  const sr = 1000, one = new Float32Array(1000).fill(1), fade = { in: 0, out: 0.1 }, fadeIn = { in: 0.1, out: 0 };
  const [m] = mixdown([{ chs: [one], sr, gain: 1, hits: [0], fades: [fade] }, { chs: [one], sr, gain: 1, hits: [0.9], fades: [fadeIn] }], sr, 1.9);
  assert.equal(m[500], 1);
  assert.equal(m[1500], 1);
  // Equal power: at the middle of the overlap each side is at 0.707, so the sum of two equal signals is 1.414, and the power is 1.
  assert.ok(Math.abs(m[950] - Math.SQRT2) < 0.01);
  assert.ok(m[905] > 0.99 && m[905] < 1.2);
});
