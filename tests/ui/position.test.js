import { test } from 'node:test';
import assert from 'node:assert/strict';

import { DEFAULT, MAX_HEIGHT, MAX_WIDTH, SNAP, clamp, dragged, isDrag } from '../../ui/position.js';

const W = 1440;
const H = 900;

test('the default sits centred in the top edge', () => {
  assert.deepEqual(clamp(DEFAULT, W, H), { x: 0.5, y: 0 });
});

test('the widest pill always stays on screen', () => {
  const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-9, `${a} != ${b}`);
  near(clamp({ x: 0, y: 300 }, W, H).x * W, MAX_WIDTH / 2);
  near(clamp({ x: 1, y: 300 }, W, H).x * W, W - MAX_WIDTH / 2);
  const low = clamp({ x: 0.5, y: 5000 }, W, H);
  assert.equal(low.y, H - MAX_HEIGHT);
});

test('near the top it snaps into the edge', () => {
  assert.equal(clamp({ x: 0.3, y: SNAP - 1 }, W, H).y, 0);
  assert.equal(clamp({ x: 0.3, y: SNAP + 1 }, W, H).y, SNAP + 1);
});

test('nonsense falls back to the default', () => {
  assert.deepEqual(clamp({ x: NaN, y: Infinity }, W, H), DEFAULT);
  assert.deepEqual(clamp({ x: 0.2, y: 0 }, 0, 0), DEFAULT);
  assert.deepEqual(clamp({ x: 0.2, y: 0 }, NaN, H), DEFAULT);
});

test('a tiny screen keeps the pill centred', () => {
  assert.equal(clamp({ x: 0, y: 0 }, 300, 150).x, 0.5);
});

test('dragging follows the pointer and never goes above the screen', () => {
  assert.deepEqual(dragged({ x: 0.5, y: 0 }, 144, 50, W), { x: 0.6, y: 50 });
  assert.deepEqual(dragged({ x: 0.5, y: 20 }, 0, -100, W), { x: 0.5, y: 0 });
});

test('a small wiggle is still a click', () => {
  assert.equal(isDrag(3, 3), false);
  assert.equal(isDrag(4, 4), true);
});
