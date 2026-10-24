// The settings window. Everything is validated and saved by the app; this page shows the
// values, collects the form, and reports what happened.

import {
  EFFORTS,
  MODELS,
  MODES,
  SOURCES,
  effective,
  lines,
  modelChoice,
  sameManaged,
  timeAgo,
  toManaged,
} from './settings-model.js';
import { frame } from './sprite.js';

const tauri = window.__TAURI__;
const $ = (id) => document.getElementById(id);
const invoke = (cmd, args) => tauri.core.invoke(cmd, args);

let view = null;

function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  Object.assign(node, props);
  node.append(...children.filter((c) => c !== null && c !== undefined));
  return node;
}

let toastTimer = 0;
function toast(message, bad = false) {
  const t = $('toast');
  t.textContent = message;
  t.classList.toggle('bad', bad);
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), bad ? 6000 : 2500);
}

/** Runs `work` with `button` disabled, and reports a failure in a toast. */
async function busy(button, work) {
  button.disabled = true;
  try {
    return await work();
  } catch (e) {
    toast(String(e), true);
    return undefined;
  } finally {
    button.disabled = false;
  }
}

// Tabs
function showTab(name) {
  for (const tab of document.querySelectorAll('.tab')) {
    if (tab.dataset.tab === name) tab.setAttribute('aria-current', 'page');
    else tab.removeAttribute('aria-current');
  }
  for (const panel of document.querySelectorAll('.panel')) panel.hidden = panel.id !== `tab-${name}`;
  if (name === 'history') loadHistory();
}
for (const tab of document.querySelectorAll('.tab')) tab.addEventListener('click', () => showTab(tab.dataset.tab));

// `claude-notch settings history` opens settings.html#history.
const TABS = ['claude', 'personality', 'voice', 'history', 'about'];
function showHashTab() {
  const name = location.hash.slice(1);
  if (TABS.includes(name)) showTab(name);
}
window.addEventListener('hashchange', showHashTab);

// Claude tab
const scope = () => document.querySelector('input[name="scope"]:checked').value;

function buildChoices() {
  for (const mode of MODES) {
    const input = el('input', { type: 'radio', name: 'mode', value: mode.value });
    $('modes').append(
      el('label', { className: 'choice' }, input, el('span', {}, el('b', { textContent: mode.title }), el('small', { textContent: mode.desc }))),
    );
  }
  for (const effort of EFFORTS) {
    const input = el('input', { type: 'radio', name: 'effort', value: effort.value });
    $('efforts').append(el('label', {}, input, el('span', { textContent: effort.label })));
  }
  for (const model of MODELS) $('model').append(el('option', { value: model.value, textContent: model.label }));
}

function check(name, value) {
  const input = document.querySelector(`input[name="${name}"][value="${CSS.escape(value ?? '')}"]`);
  if (input) input.checked = true;
}

function fillClaude() {
  const file = view[scope()];
  const settings = file.settings;
  const voice = scope() === 'voice';
  $('claude-error').hidden = !file.error;
  $('claude-error').textContent = file.error ?? '';
  for (const input of document.querySelectorAll('#tab-claude .card input, #tab-claude .card select, #tab-claude textarea, #save-claude')) {
    input.disabled = Boolean(file.error);
  }
  for (const mode of MODES) {
    const input = document.querySelector(`input[name="mode"][value="${mode.value}"]`);
    if (mode.userOnly && voice) input.disabled = true;
  }
  $('mode-note').hidden = !voice;
  check('mode', settings.mode);
  check('effort', settings.effort);
  const choice = modelChoice(settings.model);
  $('model').value = choice.select;
  $('model-custom').value = choice.custom;
  $('model-custom').hidden = choice.select !== 'custom';
  $('allow').value = (settings.allow ?? []).join('\n');
  fillEffective();
}

function claudeForm() {
  return toManaged({
    mode: document.querySelector('input[name="mode"]:checked')?.value ?? '',
    effort: document.querySelector('input[name="effort"]:checked')?.value ?? '',
    model: $('model').value,
    custom: $('model-custom').value,
    allow: $('allow').value,
  });
}

function fillEffective() {
  const voice = view.voice.settings;
  const user = view.user.settings;
  const rows = [
    ['Model', 'model', "Claude Code's default"],
    ['Effort', 'effort', "Claude Code's default"],
    ['Permission mode', 'mode', "Claude Code's default"],
  ];
  $('effective').replaceChildren(
    ...rows.flatMap(([label, key, fallback]) => {
      const { value, from } = effective(voice, user, key);
      const shown = key === 'mode' ? (MODES.find((m) => m.value === value)?.title ?? value) : value;
      return [el('dt', { textContent: label }), el('dd', {}, shown ?? fallback, el('span', { className: 'from', textContent: `(${SOURCES[from]})` }))];
    }),
    el('dt', { textContent: 'Browser' }),
    el(
      'dd',
      {},
      view.chrome ? 'Claude in Chrome on' : 'No browser control',
      el('span', { className: 'from', textContent: view.chrome ? '(from /chrome)' : '(turn on with /chrome → Enabled by default)' }),
    ),
  );
}

$('model').addEventListener('change', () => {
  $('model-custom').hidden = $('model').value !== 'custom';
  if (!$('model-custom').hidden) $('model-custom').focus();
});
for (const radio of document.querySelectorAll('input[name="scope"]')) radio.addEventListener('change', fillClaude);

$('save-claude').addEventListener('click', (event) =>
  busy(event.currentTarget, async () => {
    const settings = claudeForm();
    if (sameManaged(settings, view[scope()].settings)) {
      toast('Nothing changed.');
      return;
    }
    await invoke('save_claude', { scope: scope(), settings });
    toast(scope() === 'voice' ? 'Saved for voice commands.' : 'Saved for all Claude Code sessions.');
    await load();
  }),
);

$('open-claude-file').addEventListener('click', () =>
  invoke('open_location', { which: scope() === 'voice' ? 'voice_settings' : 'user_settings' }).catch((e) => toast(String(e), true)),
);

$('test').addEventListener('click', (event) =>
  busy(event.currentTarget, async () => {
    const out = $('test-result');
    out.hidden = false;
    out.replaceChildren(el('dt', { textContent: 'Status' }), el('dd', { textContent: 'Testing… (up to 90 seconds)' }));
    const p = await invoke('test_claude');
    const rows = [
      ['Status', el('span', { className: p.ok ? 'good' : 'bad', textContent: p.ok ? 'Connected' : 'Failed' })],
      ['Claude Code', p.version ?? '-'],
      ['Model', p.model ?? '-'],
      ['Reply', p.reply || '-'],
      ['Time', `${p.seconds.toFixed(1)} s`],
      ['Cost', `$${p.cost_usd.toFixed(4)}`],
    ];
    if (p.error) rows.push(['Problem', el('span', { className: 'bad', textContent: p.error })]);
    out.replaceChildren(...rows.flatMap(([k, v]) => [el('dt', { textContent: k }), el('dd', {}, v)]));
  }),
);

// Personality
$('save-personality').addEventListener('click', (event) =>
  busy(event.currentTarget, async () => {
    await invoke('save_personality', { text: $('personality').value });
    toast('Personality saved. It applies from the next command.');
  }),
);

// Voice & notch
function fillNotch() {
  const n = view.notch;
  $('config-path').textContent = view.paths.config;
  $('wake').value = n.wake_phrases.join('\n');
  $('language').value = n.language;
  if (!$('language').value) $('language').append(el('option', { value: n.language, textContent: n.language, selected: true }));
  const current = n.model_path ?? `${view.paths.models}/ggml-small-q5_1.bin`;
  const models = [...new Set([...view.speech_models, current])];
  $('speech-model').replaceChildren(
    ...models.map((path) => el('option', { value: path, textContent: path.split('/').pop(), selected: path === current })),
  );
  $('vad').value = n.vad_threshold;
  $('vad-value').textContent = Number(n.vad_threshold).toFixed(3);
  $('follow-up').value = n.follow_up_secs;
  $('timeout').value = n.turn_timeout_secs;
  $('sounds').checked = n.sounds;
  $('speak').checked = n.speak;
  $('claude-bin').value = n.claude_bin;
  $('restart-banner').hidden = !view.restart_needed;
}

$('vad').addEventListener('input', () => {
  $('vad-value').textContent = Number($('vad').value).toFixed(3);
});

$('save-notch').addEventListener('click', (event) =>
  busy(event.currentTarget, async () => {
    const defaultModel = `${view.paths.models}/ggml-small-q5_1.bin`;
    const notch = {
      ...view.notch,
      wake_phrases: lines($('wake').value),
      language: $('language').value,
      model_path: $('speech-model').value === defaultModel ? null : $('speech-model').value,
      vad_threshold: Number($('vad').value),
      follow_up_secs: Number.parseInt($('follow-up').value, 10),
      turn_timeout_secs: Number.parseInt($('timeout').value, 10),
      sounds: $('sounds').checked,
      speak: $('speak').checked,
      claude_bin: $('claude-bin').value.trim(),
    };
    const restart = await invoke('save_notch', { notch });
    toast(restart ? 'Saved. Some changes apply after a restart.' : 'Saved.');
    await load();
  }),
);

$('reset-position').addEventListener('click', (event) =>
  busy(event.currentTarget, async () => {
    await invoke('reset_position');
    toast('The notch is back at the top centre.');
  }),
);

// History
async function loadHistory() {
  const sessions = await invoke('list_history').catch((e) => {
    toast(String(e), true);
    return [];
  });
  const now = Date.now() / 1000;
  $('history-empty').hidden = sessions.length > 0;
  $('history').replaceChildren(
    ...sessions.map((s) => {
      const resume = el('button', { type: 'button', textContent: 'Continue in terminal' });
      resume.addEventListener('click', () =>
        busy(resume, async () => {
          await invoke('resume_session', { id: s.id });
          toast('Opening a terminal…');
        }),
      );
      const turns = s.prompts > 1 ? ` · ${s.prompts} commands` : '';
      return el(
        'li',
        {},
        el('span', { className: 'prompt', textContent: s.first_prompt }),
        resume,
        el('span', { className: 'answer', textContent: s.last_answer }),
        el('span', { className: 'meta', textContent: `${timeAgo(s.updated, now)}${turns}` }),
      );
    }),
  );
}

$('reload-history').addEventListener('click', loadHistory);

let confirmTimer = 0;
$('delete-history').addEventListener('click', (event) => {
  const button = event.currentTarget;
  if (!button.classList.contains('confirm')) {
    button.classList.add('confirm');
    button.textContent = 'Sure? Click again to delete';
    confirmTimer = setTimeout(() => {
      button.classList.remove('confirm');
      button.textContent = 'Delete all voice history';
    }, 4000);
    return;
  }
  clearTimeout(confirmTimer);
  button.classList.remove('confirm');
  button.textContent = 'Delete all voice history';
  busy(button, async () => {
    const n = await invoke('delete_history');
    toast(`${n} sessions deleted.`);
    await loadHistory();
  });
});

// About
const LOCATIONS = [
  ['Notch settings', 'config', 'config'],
  ['Personality (CLAUDE.md)', 'personality', 'personality'],
  ['Voice workspace', 'workspace', 'workspace'],
  ['Speech-to-text models', 'models', 'models'],
  ['History (transcripts)', 'history', 'history'],
  ['Settings backups', 'backups', 'backups'],
];

function fillAbout() {
  $('version').textContent = `v${view.version}`;
  $('voice-path').textContent = view.voice.path;
  $('user-path').textContent = view.user.path;
  $('paths').replaceChildren(
    ...LOCATIONS.flatMap(([label, key, which]) => {
      const open = el('button', { type: 'button', textContent: 'Open' });
      open.addEventListener('click', () => invoke('open_location', { which }).catch((e) => toast(String(e), true)));
      return [el('dt', { textContent: label }), el('dd', {}, el('span', { textContent: view.paths[key] }), open)];
    }),
  );
}

for (const button of document.querySelectorAll('[data-open]')) {
  button.addEventListener('click', () => invoke('open_location', { which: button.dataset.open }).catch((e) => toast(String(e), true)));
}

for (const button of document.querySelectorAll('[data-restart]')) {
  button.addEventListener('click', () =>
    busy(button, async () => {
      await invoke('restart_notch');
      toast('Restarting the notch…');
    }),
  );
}

function drawLogo() {
  const canvas = $('logo');
  const ctx = canvas.getContext('2d');
  for (const [x, y, color] of frame('idle', 0).pixels) {
    ctx.fillStyle = color;
    ctx.fillRect(x, y, 1, 1);
  }
}

async function load() {
  try {
    view = await invoke('get_settings');
  } catch (e) {
    toast(String(e), true);
    return;
  }
  fillAbout();
  fillClaude();
  fillNotch();
}

buildChoices();
drawLogo();
await load();
showHashTab();
$('personality').value = await invoke('read_personality').catch((e) => {
  toast(String(e), true);
  return '';
});
