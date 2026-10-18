import { test } from 'node:test';
import assert from 'node:assert/strict';

import { SOUNDS, play } from '../../ui/sfx.js';

test('every sound the app emits exists and is short and audible', () => {
  // The names come from machine.rs' Sfx enum.
  for (const name of ['wake', 'done', 'error', 'attention', 'cancel']) {
    const notes = SOUNDS[name];
    assert.ok(Array.isArray(notes) && notes.length > 0, name);
    const total = notes.reduce((sum, [, seconds]) => sum + seconds, 0);
    assert.ok(total > 0 && total <= 0.5, `${name} lasts ${total}s`);
    for (const [freq, seconds] of notes) {
      assert.ok(freq === 0 || (freq >= 100 && freq <= 4000), `${name}: ${freq} Hz`);
      assert.ok(seconds > 0 && seconds <= 0.3, `${name}: ${seconds}s`);
    }
  }
});

test('without Web Audio, playing is a silent no-op', () => {
  assert.equal(typeof globalThis.AudioContext, 'undefined');
  const warn = console.warn;
  console.warn = () => {};
  try {
    assert.doesNotThrow(() => play('wake'));
    assert.doesNotThrow(() => play('no-such-sound'));
  } finally {
    console.warn = warn;
  }
});
