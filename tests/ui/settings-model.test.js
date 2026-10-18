import { test } from 'node:test';
import assert from 'node:assert/strict';

import { EFFORTS, MODES, effective, lines, modelChoice, sameManaged, timeAgo, toManaged } from '../../ui/settings-model.js';

test('voice settings override user settings, which override the default', () => {
  const user = { model: 'opus', effort: null, mode: 'auto', allow: null };
  const voice = { model: null, effort: 'low', mode: null, allow: ['WebSearch'] };
  assert.deepEqual(effective(voice, user, 'model'), { value: 'opus', from: 'user' });
  assert.deepEqual(effective(voice, user, 'effort'), { value: 'low', from: 'voice' });
  assert.deepEqual(effective(voice, user, 'mode'), { value: 'auto', from: 'user' });
  assert.deepEqual(effective({}, {}, 'model'), { value: null, from: 'default' });
});

test('auto and bypassPermissions from the voice file are ignored, like Claude Code does', () => {
  const user = { mode: 'default' };
  assert.deepEqual(effective({ mode: 'auto' }, user, 'mode'), { value: 'default', from: 'user' });
  assert.deepEqual(effective({ mode: 'bypassPermissions' }, {}, 'mode'), { value: null, from: 'default' });
  assert.deepEqual(effective({ mode: 'acceptEdits' }, user, 'mode'), { value: 'acceptEdits', from: 'voice' });
});

test('form to saved settings', () => {
  assert.deepEqual(toManaged({ model: '', custom: '', effort: '', mode: '', allow: '  \n' }), {
    model: null,
    effort: null,
    mode: null,
    allow: null,
  });
  assert.deepEqual(
    toManaged({ model: 'custom', custom: ' claude-opus-5-5[1m] ', effort: 'low', mode: 'default', allow: 'A\n\n B \nA' }),
    { model: 'claude-opus-5-5[1m]', effort: 'low', mode: 'default', allow: ['A', 'B'] },
  );
  assert.equal(toManaged({ model: 'custom', custom: '  ', effort: '', mode: '', allow: '' }).model, null);
});

test('model choice round-trips through the form', () => {
  assert.deepEqual(modelChoice(null), { select: '', custom: '' });
  assert.deepEqual(modelChoice('opus'), { select: 'opus', custom: '' });
  assert.deepEqual(modelChoice('claude-haiku-5-5'), { select: 'custom', custom: 'claude-haiku-5-5' });
  assert.deepEqual(modelChoice('custom'), { select: 'custom', custom: 'custom' });
});

test('change detection ignores missing versus null', () => {
  assert.ok(sameManaged({ model: 'opus' }, { model: 'opus', effort: null, mode: null, allow: null }));
  assert.ok(!sameManaged({ allow: ['A'] }, { allow: ['B'] }));
});

test('lines and choices', () => {
  assert.deepEqual(lines(null), []);
  assert.deepEqual(lines('hi claude\r\n hey claude \n'), ['hi claude', 'hey claude']);
  assert.ok(MODES.find((m) => m.value === 'auto').userOnly, 'auto mode only from user settings');
  assert.ok(!MODES.some((m) => m.value === 'bypassPermissions'));
  assert.deepEqual(
    EFFORTS.map((e) => e.value),
    ['', 'low', 'medium', 'high', 'xhigh'],
  );
});

test('relative time', () => {
  assert.equal(timeAgo(1000, 1030), 'just now');
  assert.equal(timeAgo(1000, 1000 + 5 * 60), '5 min ago');
  assert.equal(timeAgo(1000, 1000 + 3 * 3600), '3 h ago');
  assert.equal(timeAgo(1000, 1000 + 86400), '1 day ago');
  assert.equal(timeAgo(1000, 1000 + 2 * 86400), '2 days ago');
  assert.equal(timeAgo(2000, 1000), 'just now');
});
