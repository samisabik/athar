// Heavy audio work runs in worker.js so the window stays responsive. run() resolves with the
// worker's result. Each lane is one worker doing its jobs in the order they were asked for;
// lane 0 is the default, and the other lanes let independent jobs (one per channel) run side by side.
export const LANES = Math.max(1, Math.min(4, navigator.hardwareConcurrency || 2));
const workers = [], waiting = new Map();
let seq = 0;

function fail(lane, msg) {
  for (const [id, w] of waiting) if (w.lane === lane) { waiting.delete(id); w.reject(new Error(msg)); }
  workers[lane]?.terminate(); workers[lane] = null;
}

function start(lane) {
  const worker = workers[lane] = new Worker('/static/worker.js', { type: 'module' });
  worker.onmessage = (e) => {
    const { id, r, error } = e.data, w = waiting.get(id);
    if (!w) return;
    waiting.delete(id);
    if (error) w.reject(new Error(error)); else w.resolve(r);
  };
  worker.onerror = (e) => fail(lane, e.message || 'Audio processing stopped unexpectedly');
  return worker;
}

export function run(op, args, lane = 0) {
  lane %= LANES;
  return new Promise((resolve, reject) => {
    const id = ++seq;
    waiting.set(id, { resolve, reject, lane });
    (workers[lane] || start(lane)).postMessage({ id, op, args });
  });
}

/** Run a job that treats every channel on its own (op must return { channels }) with the
    channels spread across the lanes. Resolves with the first lane's result, all channels joined. */
export async function runPerChannel(op, args) {
  const parts = await Promise.all(args.chs.map((d, i) => run(op, { ...args, chs: [d] }, i)));
  return { ...parts[0], channels: parts.map(p => p.channels[0]) };
}
