// The pixel buddy: a Claude-orange critter with a little spark on its head, drawn in code on a
// 17×14 grid. frame() is a pure function of (mode, time, options), so tests can check every look.

export const W = 17;
export const H = 14;

export const COLORS = Object.freeze({
  body: '#d97757',
  shade: '#b4573a',
  light: '#f0a383',
  eye: '#2b1b17',
  glint: '#ffffff',
  blush: '#f28c8c',
  spark: '#ffd7a8',
  sparkHot: '#ffffff',
  zzz: '#bcd4ff',
  ash: '#7d6f68',
  drop: '#7fc4ff',
  alert: '#ffcf4a',
  confetti: '#9be37b',
});

const BODY = { x0: 2, x1: 14, y0: 3, y1: 10 };
// Left eye columns 4–5, right 11–12: mirror images around column 8.
const EYES = [4, 11];
const LEGS = [3, 5, 11, 13];
// Resting, the buddy does one little thing every few seconds, at the end of each cycle.
const FIDGET_EVERY = 6;
const FIDGET_FOR = 1.5;
const FIDGETS = ['wave', 'hop', 'look', 'twinkle'];

const step = (t, hz) => Math.floor(t * hz);

class Grid {
  constructor() {
    this.cells = new Map();
  }

  set(x, y, color) {
    if (x >= 0 && x < W && y >= 0 && y < H) this.cells.set(y * W + x, color);
  }

  pixels() {
    return [...this.cells].map(([key, color]) => [key % W, Math.floor(key / W), color]);
  }
}

/** The fidget a resting buddy is doing at time t, or null. */
export function fidget(t) {
  if (t % FIDGET_EVERY < FIDGET_EVERY - FIDGET_FOR) return null;
  return FIDGETS[Math.floor(t / FIDGET_EVERY) % FIDGETS.length];
}

function body(g) {
  for (let y = BODY.y0; y <= BODY.y1; y++) {
    for (let x = BODY.x0; x <= BODY.x1; x++) {
      const corner = (x === BODY.x0 || x === BODY.x1) && (y === BODY.y0 || y === BODY.y1);
      if (corner) continue;
      let color = COLORS.body;
      if (y === BODY.y0) color = COLORS.light;
      else if (y === BODY.y1 || x === BODY.x1) color = COLORS.shade;
      g.set(x, y, color);
    }
  }
}

function arms(g, leftUp, rightUp) {
  for (const [x, up] of [[0, leftUp], [15, rightUp]]) {
    const y = up ? 4 : 6;
    g.set(x, y, COLORS.body);
    g.set(x + 1, y, COLORS.body);
    g.set(x, y + 1, COLORS.shade);
    g.set(x + 1, y + 1, COLORS.shade);
  }
}

// phase 0: all legs down; 1 and 2: alternate pairs lifted, for walking.
function legs(g, phase) {
  LEGS.forEach((x, i) => {
    g.set(x, 11, COLORS.shade);
    if (phase === 0 || i % 2 !== phase - 1) g.set(x, 12, COLORS.shade);
  });
}

// The spark on its head: 'plus', 'cross' (twinkling), 'out' (an error put it out) or 'alert' (!).
function spark(g, look) {
  if (look === 'alert') {
    for (const y of [0, 1]) g.set(8, y, COLORS.alert);
    return; // the dot of the "!" is the head itself
  }
  const plus = [[8, 0], [7, 1], [8, 1], [9, 1], [8, 2]];
  const cross = [[7, 0], [9, 0], [8, 1], [7, 2], [9, 2]];
  const color = { plus: COLORS.spark, cross: COLORS.sparkHot, out: COLORS.ash }[look];
  for (const [x, y] of look === 'cross' ? cross : plus) g.set(x, y, color);
}

function eyes(g, style, dx, dy) {
  for (const [i, x0] of EYES.entries()) {
    const x = x0 + dx;
    const left = i === 0;
    switch (style) {
      case 'closed':
        g.set(x, 6, COLORS.eye);
        g.set(x + 1, 6, COLORS.eye);
        break;
      case 'happy': // ^ ^
        g.set(x - 1, 6, COLORS.eye);
        g.set(x, 5, COLORS.eye);
        g.set(x + 1, 5, COLORS.eye);
        g.set(x + 2, 6, COLORS.eye);
        break;
      case 'x': {
        const a = left ? 4 : 10;
        for (const [px, py] of [[a, 4], [a + 2, 4], [a + 1, 5], [a, 6], [a + 2, 6]]) g.set(px, py, COLORS.eye);
        break;
      }
      case 'squint': {
        // > <
        const tip = left ? x + 1 : x;
        const back = left ? x : x + 1;
        g.set(back, 4, COLORS.eye);
        g.set(tip, 5, COLORS.eye);
        g.set(back, 6, COLORS.eye);
        break;
      }
      default: {
        const top = (style === 'wide' ? 4 : 5) + dy;
        for (let y = top; y <= 6 + dy; y++) {
          g.set(x, y, COLORS.eye);
          g.set(x + 1, y, COLORS.eye);
        }
        g.set(x, top, COLORS.glint);
      }
    }
  }
}

function mouth(g, style) {
  const shapes = {
    smile: [[6, 8], [7, 9], [8, 9], [9, 9], [10, 8]],
    o: [[8, 8]],
    O: [[7, 8], [8, 8], [7, 9], [8, 9]],
    flat: [[7, 9], [8, 9], [9, 9]],
    wavy: [[6, 9], [7, 8], [8, 9], [9, 8], [10, 9]],
  };
  for (const [x, y] of shapes[style] ?? []) g.set(x, y, COLORS.eye);
}

function zzz(g, t) {
  if (step(t, 1.5) % 3 === 2) return;
  const oy = step(t, 1.5) % 3;
  for (const [x, y] of [[0, 0], [1, 0], [2, 0], [1, 1], [0, 2], [1, 2], [2, 2]]) g.set(14 + x, y + 1 - oy, COLORS.zzz);
}

// A thought bubble of dots that fills up and starts again.
function thoughts(g, t) {
  const shown = step(t, 3) % 4;
  for (let i = 0; i < shown; i++) g.set(12 + i * 2, 1 - (i % 2), COLORS.spark);
}

// Two specks of confetti that hop between spots around the head.
function confetti(g, t) {
  const spots = [[1, 1], [15, 0], [3, 0], [13, 2], [0, 3], [16, 2]];
  const i = step(t, 5);
  g.set(...spots[i % spots.length], COLORS.confetti);
  g.set(...spots[(i + 3) % spots.length], COLORS.sparkHot);
}

function sweat(g, t) {
  const y = 3 + (step(t, 4) % 3);
  g.set(15, y, COLORS.drop);
}

/**
 * @param {string} mode notch mode: idle, sleep, paused, listening, transcribing, thinking,
 *   working, approval, done, error. Anything else looks idle.
 * @param {number} t seconds since start
 * @param {{blink?: boolean, look?: number, poke?: boolean, level?: number, talk?: boolean}} opt
 *   talk: it is reading the answer aloud.
 * @returns {{pixels: Array<[number, number, string]>, dx: number, dy: number}}
 */
export function frame(mode, t, opt = {}) {
  const { blink = false, look = 0, poke = false, level = 0, talk = false } = opt;
  const g = new Grid();
  let leftUp = false;
  let rightUp = false;
  let legPhase = 0;
  let dx = 0;
  let dy = 0;
  let eyeStyle = 'open';
  let mouthStyle = null;
  let eyeX = Math.max(-1, Math.min(1, Math.round(look)));
  let eyeY = 0;
  let sparkLook = 'plus';
  let extra = null;

  switch (mode) {
    case 'listening':
      eyeStyle = 'wide';
      mouthStyle = level > 0.08 ? 'O' : 'o';
      leftUp = rightUp = level > 0.04;
      dy = level > 0.15 ? -1 : 0;
      sparkLook = level > 0.04 && step(t, 8) % 2 === 1 ? 'cross' : 'plus';
      break;
    case 'transcribing':
      eyeStyle = 'wide';
      mouthStyle = 'o';
      eyeX = [-1, 0, 1, 0][step(t, 4) % 4]; // reading back what it heard
      leftUp = step(t, 4) % 2 === 0;
      rightUp = !leftUp;
      break;
    case 'thinking':
      eyeX = step(t, 0.5) % 2 === 0 ? 1 : -1;
      eyeY = -1;
      dy = step(t, 2) % 2;
      mouthStyle = step(t, 1) % 3 === 2 ? 'wavy' : null;
      sparkLook = step(t, 4) % 2 === 1 ? 'cross' : 'plus';
      rightUp = true; // hand on chin
      extra = thoughts;
      break;
    case 'working':
      legPhase = 1 + (step(t, 6) % 2);
      leftUp = step(t, 6) % 2 === 0;
      rightUp = !leftUp;
      sparkLook = step(t, 6) % 2 === 1 ? 'cross' : 'plus';
      dx = [0, 1, 0, -1][step(t, 1.5) % 4]; // pacing back and forth
      dy = -(step(t, 6) % 2);
      eyeX = dx;
      break;
    case 'approval':
      eyeStyle = 'wide';
      mouthStyle = 'o';
      rightUp = step(t, 3) % 2 === 0;
      sparkLook = step(t, 2) % 2 === 0 ? 'alert' : 'plus';
      break;
    case 'done':
      eyeStyle = 'happy';
      mouthStyle = 'smile';
      dy = t % 0.8 < 0.2 ? -1 : 0;
      leftUp = rightUp = dy < 0;
      extra = confetti;
      break;
    case 'error':
      eyeStyle = 'x';
      mouthStyle = 'wavy';
      dx = t % 1.5 < 0.4 ? step(t, 12) % 2 : 0;
      sparkLook = 'out';
      extra = sweat;
      break;
    case 'sleep':
    case 'paused':
      eyeStyle = 'closed';
      dy = mode === 'sleep' ? 1 - (step(t, 0.5) % 2) : 1; // slow, deep breaths
      break;
    default: {
      dy = step(t, 1 / 1.4) % 2; // breathing
      const now = fidget(t);
      if (now === 'wave') rightUp = step(t, 4) % 2 === 0;
      else if (now === 'hop') {
        dy = t % 0.5 < 0.2 ? -1 : 0;
        leftUp = rightUp = dy < 0;
      } else if (now === 'look' && look === 0) eyeX = t % FIDGET_EVERY < FIDGET_EVERY - FIDGET_FOR / 2 ? -1 : 1;
      else if (now === 'twinkle') sparkLook = step(t, 6) % 2 === 1 ? 'cross' : 'plus';
    }
  }
  if (talk) {
    // Reading the answer aloud: the mouth moves with a syllable rhythm, one hand talks along.
    eyeStyle = eyeStyle === 'x' ? eyeStyle : 'open';
    mouthStyle = ['o', 'O', 'flat', 'O', 'smile'][step(t, 7) % 5];
    leftUp = step(t, 2) % 2 === 1;
    extra = null;
  }
  if (poke) {
    eyeStyle = 'squint';
    mouthStyle = 'o';
  } else if (blink && (eyeStyle === 'open' || eyeStyle === 'wide')) {
    eyeStyle = 'closed';
  }

  body(g);
  arms(g, leftUp, rightUp);
  legs(g, legPhase);
  spark(g, sparkLook);
  if (eyeStyle !== 'x') {
    g.set(3, 8, COLORS.blush);
    g.set(13, 8, COLORS.blush);
  }
  eyes(g, eyeStyle, eyeStyle === 'open' || eyeStyle === 'wide' ? eyeX : 0, eyeY);
  mouth(g, mouthStyle);
  if (mode === 'sleep') zzz(g, t);
  extra?.(g, t);
  return { pixels: g.pixels(), dx, dy };
}
