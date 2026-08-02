// Dev-only headless check for the UI effects bus (sfx.ts). NOT shipped:
// nothing imports it, so the bundle never sees it.
//
//   pnpm --filter @ee/app sfxcheck
//
// `audiocheck` proves things about the WORKLET — the path that carries real
// solver samples — and it deliberately knows nothing about this file. So the
// break sound needs its own harness. CI has no ears, but it can hold the
// node graph up to the light, and these are the properties that matter and
// that a browser would not tell you reliably:
//
//   • ISOLATION: the effect chain ends at the destination it was handed and
//     touches nothing else. No worklet node, no ring buffer, no postMessage.
//   • ROUTING: every voice goes through the bus gain AND the effects' own
//     compressor, so master mute is exact silence and a pile-up cannot clip.
//   • THE CAP: MAX_VOICES simultaneous breaks play, the rest are dropped,
//     and the slots come back when the voices end.
//   • SELECTION: a kind with a bespoke voice gets it; an unknown, absent or
//     misspelled kind gets the generic one instead of nothing.
//   • SANITY: no NaN, no gain above full scale, no exponential ramp through
//     zero (which silently kills a WebAudio param), every envelope ending at
//     exactly 0 so no voice leaves DC on the bus.
//
// What it CANNOT prove: that a capacitor sounds like a capacitor. That needs
// a browser, a speaker and a human.

import { DEFAULT_VOICE, MAX_VOICES, SfxBus, VOICES, voiceFor } from './sfx';

// ------------------------------------------------------------------- stubs

const SAMPLE_RATE = 48000;

type EvKind = 'set' | 'lin' | 'exp' | 'cancel';
interface Ev {
  k: EvKind;
  v: number;
  t: number;
}

class StubParam {
  value = 0;
  readonly ev: Ev[] = [];
  constructor(readonly name: string) {}
  setValueAtTime(v: number, t: number) {
    this.ev.push({ k: 'set', v, t });
  }
  linearRampToValueAtTime(v: number, t: number) {
    this.ev.push({ k: 'lin', v, t });
  }
  exponentialRampToValueAtTime(v: number, t: number) {
    this.ev.push({ k: 'exp', v, t });
  }
  cancelScheduledValues(t: number) {
    this.ev.push({ k: 'cancel', v: 0, t });
  }
}

class StubNode {
  readonly out: StubNode[] = [];
  constructor(
    readonly kind: string,
    readonly ctx: StubCtx,
  ) {}
  connect(n: StubNode) {
    this.out.push(n);
    return n;
  }
  disconnect() {
    this.out.length = 0;
  }
}

class StubGain extends StubNode {
  readonly gain = new StubParam('gain');
  constructor(ctx: StubCtx) {
    super('gain', ctx);
  }
}
class StubBiquad extends StubNode {
  type = 'lowpass';
  readonly frequency = new StubParam('frequency');
  readonly Q = new StubParam('Q');
  readonly detune = new StubParam('detune');
  constructor(ctx: StubCtx) {
    super('biquad', ctx);
  }
}
class StubOsc extends StubNode {
  type = 'sine';
  readonly frequency = new StubParam('frequency');
  readonly detune = new StubParam('detune');
  started: number | null = null;
  stopped: number | null = null;
  constructor(ctx: StubCtx) {
    super('osc', ctx);
  }
  start(t: number) {
    this.started = t;
  }
  stop(t: number) {
    this.stopped = t;
  }
}
class StubBufSrc extends StubNode {
  buffer: StubBuffer | null = null;
  started: number | null = null;
  offset = 0;
  stopped: number | null = null;
  constructor(ctx: StubCtx) {
    super('bufsrc', ctx);
  }
  start(t: number, off = 0) {
    this.started = t;
    this.offset = off;
  }
  stop(t: number) {
    this.stopped = t;
  }
}
class StubComp extends StubNode {
  readonly threshold = new StubParam('threshold');
  readonly knee = new StubParam('knee');
  readonly ratio = new StubParam('ratio');
  readonly attack = new StubParam('attack');
  readonly release = new StubParam('release');
  constructor(ctx: StubCtx) {
    super('comp', ctx);
  }
}
class StubBuffer {
  readonly data: Float32Array;
  constructor(
    readonly channels: number,
    readonly length: number,
    readonly sampleRate: number,
  ) {
    this.data = new Float32Array(length);
  }
  getChannelData() {
    return this.data;
  }
}

class StubCtx {
  currentTime = 0;
  readonly sampleRate = SAMPLE_RATE;
  readonly nodes: StubNode[] = [];
  buffers = 0;
  readonly destination: StubNode;
  constructor() {
    this.destination = new StubNode('destination', this);
  }
  private track<T extends StubNode>(n: T): T {
    this.nodes.push(n);
    return n;
  }
  createGain() {
    return this.track(new StubGain(this));
  }
  createBiquadFilter() {
    return this.track(new StubBiquad(this));
  }
  createOscillator() {
    return this.track(new StubOsc(this));
  }
  createBufferSource() {
    return this.track(new StubBufSrc(this));
  }
  createDynamicsCompressor() {
    return this.track(new StubComp(this));
  }
  createBuffer(ch: number, len: number, sr: number) {
    this.buffers++;
    return new StubBuffer(ch, len, sr);
  }
}

// ------------------------------------------------------------------ runner

let failures = 0;
let checks = 0;
function ok(cond: boolean, what: string, detail = '') {
  checks++;
  if (cond) return;
  failures++;
  console.error(`  FAIL  ${what}${detail ? ` — ${detail}` : ''}`);
}
function section(name: string) {
  console.log(`\n${name}`);
}

const makeBus = () => {
  const ctx = new StubCtx();
  const bus = new SfxBus(
    ctx as unknown as BaseAudioContext,
    ctx.destination as unknown as AudioNode,
  );
  return { ctx, bus };
};

/** Every node reachable from `n`, following connections. */
function reach(n: StubNode, seen = new Set<StubNode>()): Set<StubNode> {
  if (seen.has(n)) return seen;
  seen.add(n);
  for (const o of n.out) reach(o, seen);
  return seen;
}

/** Source nodes (things that make sound) created since index `from`. */
const sourcesSince = (ctx: StubCtx, from: number) =>
  ctx.nodes.slice(from).filter((n) => n.kind === 'osc' || n.kind === 'bufsrc');

// ---------------------------------------------------------------- routing

section('routing: the effect bus meets the solver stream only at the destination');
{
  const { ctx, bus } = makeBus();
  bus.setGain(0.8);
  bus.playBreak('Resistor', 7);

  const gains = ctx.nodes.filter((n) => n.kind === 'gain') as StubGain[];
  const busGain = gains[0]!;
  const comps = ctx.nodes.filter((n) => n.kind === 'comp');
  ok(comps.length === 1, 'the bus has exactly one compressor of its own', `got ${comps.length}`);

  // Everything the bus can reach.
  const downstream = reach(busGain);
  ok(downstream.has(comps[0]!), 'bus gain feeds the compressor');
  ok(downstream.has(ctx.destination), 'the chain reaches the destination');
  ok(downstream.size === 3, 'nothing else is downstream of the bus', `${downstream.size} nodes`);

  // Every voice node must pass THROUGH the bus gain to be heard, so master
  // mute is exact silence and the compressor sees the whole sum.
  for (const s of sourcesSince(ctx, 0)) {
    ok(reach(s).has(busGain), `${s.kind} routes through the bus gain`);
  }
  // The only node connected straight to the destination is the compressor.
  const direct = ctx.nodes.filter((n) => n.out.includes(ctx.destination));
  ok(
    direct.length === 1 && direct[0]!.kind === 'comp',
    'only the compressor touches the destination',
    direct.map((d) => d.kind).join(','),
  );
}

section('mute: master gain 0 is scheduled as an exact zero');
{
  const { ctx, bus } = makeBus();
  bus.setGain(0);
  const busGain = ctx.nodes.find((n) => n.kind === 'gain') as StubGain;
  const last = busGain.gain.ev[busGain.gain.ev.length - 1]!;
  ok(last.k === 'set' && last.v === 0, 'setGain(0) schedules exactly 0', JSON.stringify(last));
  bus.setGain(1);
  const hot = busGain.gain.ev[busGain.gain.ev.length - 1]!;
  ok(hot.v > 0 && hot.v <= 1, 'setGain(1) stays inside full scale', String(hot.v));
}

// ------------------------------------------------------------------- cap

section(`voice cap: at most ${MAX_VOICES} simultaneous breaks`);
{
  const { ctx, bus } = makeBus();
  bus.setGain(1);
  const played: boolean[] = [];
  for (let i = 0; i < 9; i++) played.push(bus.playBreak('Capacitor', i));
  ok(
    played.filter(Boolean).length === MAX_VOICES,
    `${MAX_VOICES} of 9 simultaneous breaks play`,
    `${played.filter(Boolean).length} played`,
  );
  ok(played.slice(0, MAX_VOICES).every(Boolean), 'the first ones are the ones kept');
  ok(bus.voices() === MAX_VOICES, 'the bus reports its live voices', String(bus.voices()));

  // Staggered, not phase-coherent: no two voices start at the same instant.
  const starts = sourcesSince(ctx, 0)
    .map((n) => (n as StubOsc).started ?? (n as StubBufSrc).started ?? 0)
    .sort((a, b) => a - b);
  ok(starts[starts.length - 1]! > starts[0]!, 'the burst is spread in time');

  // Slots come back when the voices end.
  ctx.currentTime += 5;
  ok(bus.voices() === 0, 'voices expire', String(bus.voices()));
  ok(bus.playBreak('Capacitor', 99), 'the bus plays again afterwards');
}

section('burst ducking: later voices in a burst are quieter than the first');
{
  const { ctx, bus } = makeBus();
  bus.setGain(1);
  const peakOf = (from: number) => {
    let p = 0;
    for (const n of ctx.nodes.slice(from)) {
      if (n.kind !== 'gain') continue;
      for (const e of (n as StubGain).gain.ev) p = Math.max(p, e.v);
    }
    return p;
  };
  const a0 = ctx.nodes.length;
  bus.playBreak('Capacitor', 1);
  const first = peakOf(a0);
  const a1 = ctx.nodes.length;
  bus.playBreak('Capacitor', 1);
  const second = peakOf(a1);
  ok(second < first, 'the second simultaneous break is ducked', `${first} -> ${second}`);
}

// ------------------------------------------------------------- selection

section('selection: bespoke where declared, generic everywhere else');
{
  ok(voiceFor('Capacitor').label === 'capacitor', 'Capacitor gets its own voice');
  ok(voiceFor('Lamp').label === 'lamp', 'Lamp gets its own voice');
  ok(voiceFor('Resistor').label === 'resistor', 'Resistor gets its own voice');
  ok(voiceFor('Npn').label === 'semiconductor', 'Npn shares the semiconductor voice');
  ok(voiceFor('Motor').label === 'motor', 'the server-only motor fixture has a voice');

  ok(voiceFor(undefined) === DEFAULT_VOICE, 'an unnameable part gets the generic snap');
  ok(voiceFor('Switch') === DEFAULT_VOICE, 'a kind with no entry gets the generic snap');
  ok(voiceFor('Capacitorr') === DEFAULT_VOICE, 'a typo gets the generic snap, not silence');
  ok(voiceFor('') === DEFAULT_VOICE, 'an empty tag gets the generic snap');

  // The four the brief names must be audibly different animals.
  const four = ['Lamp', 'Npn', 'Resistor', 'Capacitor'].map((k) => voiceFor(k).label);
  ok(new Set(four).size === 4, 'lamp/semiconductor/resistor/capacitor are four voices', four.join());

  // Kinds that cannot break must not have voices (crates/damage rating()).
  for (const k of ['Wire', 'Ground', 'OpAmp', 'Ota']) {
    ok(
      !(k in VOICES),
      `${k} has no break voice (it cannot break)`,
    );
  }
}

section('selection reaches the graph: different kinds build different voices');
{
  const shape = (kind: string) => {
    const { ctx, bus } = makeBus();
    bus.setGain(1);
    bus.playBreak(kind, 3);
    return ctx.nodes
      .filter((n) => n.kind === 'osc' || n.kind === 'bufsrc' || n.kind === 'biquad')
      .map((n) => (n.kind === 'osc' ? `osc:${(n as StubOsc).type}` : n.kind))
      .join('|');
  };
  const lamp = shape('Lamp');
  const cap = shape('Capacitor');
  const semi = shape('Diode');
  const res = shape('Resistor');
  const def = shape('Switch');
  ok(new Set([lamp, cap, semi, res]).size === 4, 'four kinds, four graphs');
  ok(lamp.split('osc').length - 1 > 1, 'the lamp scatters glass shards', lamp);
  ok(semi !== def, 'silicon does not sound like the generic snap');
  ok(shape('Switch') === shape('Potentiometer'), 'both fallbacks are the same generic snap');
}

// ----------------------------------------------------------------- sanity

section('sanity: every voice in the palette schedules legal, finite audio');
{
  const kinds = [...Object.keys(VOICES), 'Switch', 'nonsense'];
  for (const kind of kinds) {
    const { ctx, bus } = makeBus();
    bus.setGain(1);
    const played = bus.playBreak(kind, 12345);
    ok(played, `${kind}: plays`);

    const srcs = sourcesSince(ctx, 0);
    ok(srcs.length > 0, `${kind}: makes at least one sound source`);

    let worst = '';
    let bad = 0;
    let zeroEnded = 0;
    let ampGains = 0;
    for (const n of ctx.nodes) {
      const params: StubParam[] = [];
      if (n.kind === 'gain') params.push((n as StubGain).gain);
      if (n.kind === 'biquad') {
        params.push((n as StubBiquad).frequency, (n as StubBiquad).Q);
      }
      if (n.kind === 'osc') params.push((n as StubOsc).frequency);
      for (const p of params) {
        for (const e of p.ev) {
          if (!Number.isFinite(e.v) || !Number.isFinite(e.t)) {
            bad++;
            worst = `${p.name} NaN/∞ ${JSON.stringify(e)}`;
          }
          // An exponential ramp to (or from) zero silently freezes a param.
          if (e.k === 'exp' && e.v <= 0) {
            bad++;
            worst = `${p.name} exp ramp to ${e.v}`;
          }
          if (p.name === 'gain' && (e.v < 0 || e.v > 1)) {
            bad++;
            worst = `gain out of range ${e.v}`;
          }
          if (p.name === 'frequency' && e.k !== 'cancel') {
            if (e.v <= 0 || e.v > SAMPLE_RATE / 2) {
              bad++;
              worst = `frequency ${e.v} outside (0, Nyquist]`;
            }
          }
        }
      }
      // Every per-voice envelope must land on an exact 0: an exponential
      // decay alone leaves a tiny DC offset running forever.
      if (n.kind === 'gain' && n !== ctx.nodes[0]) {
        ampGains++;
        const ev = (n as StubGain).gain.ev;
        const last = ev[ev.length - 1];
        if (!last || last.k !== 'lin' || last.v !== 0) zeroEnded++;
      }
    }
    ok(bad === 0, `${kind}: all scheduled values legal`, worst);
    ok(ampGains > 0 && zeroEnded === 0, `${kind}: every envelope ends at exact zero`);

    // Sources are one-shots: each starts and stops.
    const dangling = srcs.filter(
      (s) => (s as StubOsc).stopped === null && (s as StubBufSrc).stopped === null,
    );
    ok(dangling.length === 0, `${kind}: no source is left running`, String(dangling.length));

    // One shared noise buffer per bus, not one per break.
    ok(ctx.buffers <= 1, `${kind}: at most one noise buffer allocated`, String(ctx.buffers));
  }
}

section('sanity: the noise buffer is made once and reused across breaks');
{
  const { ctx, bus } = makeBus();
  bus.setGain(1);
  for (let i = 0; i < 3; i++) {
    bus.playBreak('Resistor', i);
    ctx.currentTime += 3;
  }
  ok(ctx.buffers === 1, 'three breaks, one noise buffer', String(ctx.buffers));
}

section('sanity: the same element id always sounds the same');
{
  const capture = (id: number) => {
    const { ctx, bus } = makeBus();
    bus.setGain(1);
    bus.playBreak('Resistor', id);
    return ctx.nodes
      .filter((n) => n.kind === 'osc')
      .map((n) => (n as StubOsc).frequency.ev.map((e) => e.v.toFixed(4)).join(','))
      .join('|');
  };
  ok(capture(41) === capture(41), 'deterministic per id');
  ok(capture(41) !== capture(42), 'different parts are detuned differently');
}

// ------------------------------------------------------------------ verdict

console.log(
  `\n${failures === 0 ? 'PASS' : 'FAIL'}: ${checks - failures}/${checks} checks` +
    `\n(what this cannot tell you: whether a capacitor sounds like a capacitor)`,
);
(globalThis as unknown as { process: { exitCode: number } }).process.exitCode = failures === 0 ? 0 : 1;
