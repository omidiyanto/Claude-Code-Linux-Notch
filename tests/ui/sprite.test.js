import { test } from 'node:test';
import assert from 'node:assert/strict';

import { COLORS, H, W, frame } from '../../ui/sprite.js';

const MODES = ['idle', 'sleep', 'paused', 'listening', 'transcribing', 'thinking', 'working', 'approval', 'done', 'error'];
const PALETTE = new Set(Object.values(COLORS));
const at = (f) => new Map(f.pixels.map(([x, y, c]) => [`${x},${y}`, c]));
const eyeCount = (f) => f.pixels.filter(([, , c]) => c === COLORS.eye || c === COLORS.glint).length;

test('every mode stays on the grid, in the palette, without duplicate cells', () => {
  for (const mode of MODES) {
    for (const t of [0, 0.1, 0.37, 1.2, 5.5]) {
      const f = frame(mode, t, { level: 0.2 });
      const seen = new Set();
      for (const [x, y, c] of f.pixels) {
        assert.ok(x >= 0 && x < W && y >= 0 && y < H, `${mode}: ${x},${y}`);
        assert.ok(PALETTE.has(c), `${mode}: ${c}`);
        assert.ok(!seen.has(`${x},${y}`), `${mode}: duplicate ${x},${y}`);
        seen.add(`${x},${y}`);
      }
      assert.ok(Math.abs(f.dy) <= 1 && Math.abs(f.dx) <= 1, `${mode} offsets`);
    }
  }
});

test('the resting buddy is symmetric', () => {
  const cells = at(frame('idle', 0));
  for (const key of cells.keys()) {
    const [x, y] = key.split(',').map(Number);
    assert.ok(cells.has(`${W - 1 - x},${y}`), `no mirror for ${key}`);
  }
});

test('unknown modes look idle', () => {
  assert.deepEqual(frame('???', 2), frame('idle', 2));
});

test('blinking closes the eyes', () => {
  assert.ok(eyeCount(frame('idle', 0, { blink: true })) < eyeCount(frame('idle', 0)));
  assert.ok(eyeCount(frame('listening', 0, { blink: true })) < eyeCount(frame('listening', 0)));
});

test('looking shifts the eyes, clamped to one cell', () => {
  const left = frame('idle', 0, { look: -5 });
  const right = frame('idle', 0, { look: 1 });
  const eyeXs = (f) => f.pixels.filter(([, , c]) => c === COLORS.eye).map(([x]) => x);
  assert.equal(Math.min(...eyeXs(left)), 3);
  assert.equal(Math.max(...eyeXs(right)), 13);
});

test('poke squints even mid-blink', () => {
  const squint = at(frame('idle', 0, { poke: true, blink: true }));
  assert.equal(squint.get('5,5'), COLORS.eye);
  assert.equal(squint.get('11,5'), COLORS.eye);
});

test('listening raises the arms only when there is sound', () => {
  assert.equal(at(frame('listening', 0, { level: 0 })).get('0,4'), undefined);
  assert.equal(at(frame('listening', 0, { level: 0.3 })).get('0,4'), COLORS.body);
});

test('working walks: legs alternate over time', () => {
  assert.notDeepEqual(frame('working', 0).pixels, frame('working', 1 / 6).pixels);
});

test('done hops and smiles; error shows crossed eyes and no blush', () => {
  assert.equal(frame('done', 0.1).dy, -1);
  assert.equal(frame('done', 0.5).dy, 0);
  const error = at(frame('error', 2));
  assert.equal(error.get('4,4'), COLORS.eye);
  assert.ok(![...error.values()].includes(COLORS.blush));
});

test('only sleep shows the z', () => {
  const hasZ = (mode, t) => frame(mode, t).pixels.some(([, , c]) => c === COLORS.zzz);
  assert.ok(hasZ('sleep', 0));
  assert.ok(!hasZ('paused', 0));
  assert.ok(!hasZ('idle', 0));
});
