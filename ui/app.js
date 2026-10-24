// The notch page: draws what the app says (it holds no logic of its own), animates the buddy,
// places the pill where the user dragged it, and reports its rectangle so only the pill takes
// clicks. At rest it hides; hovering where it hides, or anything happening, brings it back.

import { DEFAULT, clamp, dragged, isDrag } from './position.js';
import { frame, H } from './sprite.js';
import { play } from './sfx.js';

const tauri = window.__TAURI__;

const LABELS = {
  listening: 'Listening…',
  transcribing: 'Catching that…',
  thinking: 'Thinking…',
  working: 'Working…',
  approval: 'Allow this?',
  done: 'Done',
  error: 'Something went wrong',
};

const island = document.getElementById('island');
const canvas = document.getElementById('buddy');
const status = document.getElementById('status');
const title = document.getElementById('title');
const detail = document.getElementById('detail');
const bars = [...document.querySelectorAll('#wave i')];
const ctx = canvas.getContext('2d');
canvas.height = H + 2; // one row of room above and below for hops

let ui = { mode: 'idle', title: '', detail: '', approval: null, paused: false, follow_up: false, speaking: false };
let level = 0;
let hovered = false;
let hideTimer = 0;
// Hidden at the top edge, this many pixels of the pill stay on screen to catch the pointer.
const PEEK = 3;
const HIDE_AFTER_MS = 700;

function invoke(cmd, args) {
  return tauri.core.invoke(cmd, args).catch((e) => console.error(cmd, e));
}

function render(next) {
  ui = next;
  const resting = ui.mode === 'idle' || ui.mode === 'sleep';
  island.dataset.mode = ui.paused && resting ? 'paused' : ui.mode;
  island.classList.toggle('follow-up', ui.follow_up);
  island.classList.toggle('speaking', ui.speaking);
  updateHidden();
  status.textContent = LABELS[ui.mode] ?? '';
  title.textContent = ui.title;
  detail.textContent = ui.detail;
  if (ui.mode !== 'listening' && ui.mode !== 'transcribing') showLevel(0);
}

function showLevel(value) {
  level = value;
  const loud = Math.min(1, value * 14);
  island.style.setProperty('--level', loud.toFixed(2));
  bars.forEach((bar, i) => {
    const shape = [0.55, 0.85, 1, 0.8, 0.6][i];
    bar.style.height = `${3 + loud * 16 * shape}px`;
  });
}

// Animation: 30 fps while active, 12 fps resting (breathing and blinks need no more), 4 fps asleep.
const start = performance.now();
let lastDraw = 0;
let blinkUntil = 0;
let nextBlink = 2;
let glance = 0;
let glanceUntil = 0;
let pointerLook = null;
let pokeUntil = 0;
const seconds = () => (performance.now() - start) / 1000;

function draw(t, mode) {
  if (t > nextBlink) {
    blinkUntil = t + 0.14;
    nextBlink = t + 2 + Math.random() * 4;
  }
  if (t > glanceUntil) {
    glance = Math.random() < 0.3 ? (Math.random() < 0.5 ? -1 : 1) : 0;
    glanceUntil = t + 1 + Math.random() * 4;
  }
  const f = frame(mode, t, {
    blink: t < blinkUntil,
    look: pointerLook ?? glance,
    poke: t < pokeUntil,
    level,
    talk: ui.speaking,
  });
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  for (const [x, y, color] of f.pixels) {
    ctx.fillStyle = color;
    ctx.fillRect(x + f.dx, y + f.dy + 1, 1, 1);
  }
}

function tick() {
  const mode = island.dataset.mode;
  // Hidden, nothing needs drawing: wake up a few times a second to notice it is back.
  const sleepy = mode === 'sleep' || mode === 'paused' || island.classList.contains('hidden');
  const now = performance.now();
  const interval = sleepy ? 250 : mode === 'idle' ? 83 : 33;
  if (!document.hidden && now - lastDraw >= interval) {
    lastDraw = now;
    draw(seconds(), mode);
  }
  if (sleepy) setTimeout(() => requestAnimationFrame(tick), 250);
  else requestAnimationFrame(tick);
}

// Only the pill takes clicks: the app turns this rectangle into the window's input region.
let regionQueued = false;
function reportRegion() {
  if (regionQueued) return;
  regionQueued = true;
  requestAnimationFrame(() => {
    regionQueued = false;
    // The box the pill is settling into, not where an animation has it now, so the pointer never
    // falls out of it mid-animation.
    const width = island.offsetWidth;
    const height = island.offsetHeight;
    const x = island.offsetLeft - width / 2;
    const docked = position.y === 0;
    const peek = island.classList.contains('hidden') && docked;
    invoke('set_hit_region', { x, y: island.offsetTop, width, height: peek ? PEEK : height });
  });
}

// Hidden while resting, unless the pointer is on it, it is being dragged, or it is speaking.
function updateHidden() {
  const resting = ['idle', 'sleep', 'paused'].includes(island.dataset.mode);
  const hide = resting && !hovered && !drag && !ui.speaking;
  if (hide === island.classList.contains('hidden')) return;
  island.classList.toggle('hidden', hide);
  island.setAttribute('aria-hidden', String(hide));
  reportRegion();
}

function pointerIn() {
  clearTimeout(hideTimer);
  if (hovered) return;
  hovered = true;
  updateHidden();
}
island.addEventListener('mouseenter', pointerIn);

function pointerLeft() {
  clearTimeout(hideTimer);
  hideTimer = setTimeout(() => {
    hovered = false;
    updateHidden();
  }, HIDE_AFTER_MS);
}
island.addEventListener('mouseleave', pointerLeft);

// Position and dragging. The window covers the whole screen; the pill moves inside it.
let position = { ...DEFAULT };
let drag = null;
let justDragged = false;

function place(next) {
  position = clamp(next, window.innerWidth, window.innerHeight);
  island.style.left = `${position.x * 100}%`;
  island.style.top = `${position.y}px`;
  // Away from the top edge it is a free-floating island, rounded all around.
  island.classList.toggle('floating', position.y > 0);
  reportRegion();
}

island.addEventListener('pointerdown', (event) => {
  if (event.button !== 0 || event.target.closest('button')) return;
  drag = { x: event.clientX, y: event.clientY, from: position, moving: false, pointer: event.pointerId };
});

island.addEventListener('pointermove', (event) => {
  if (!drag) return;
  const dx = event.clientX - drag.x;
  const dy = event.clientY - drag.y;
  if (!drag.moving) {
    if (!isDrag(dx, dy)) return;
    drag.moving = true;
    island.setPointerCapture(drag.pointer);
    island.classList.add('dragging');
  }
  place(dragged(drag.from, dx, dy, window.innerWidth));
});

function endDrag() {
  if (!drag) return;
  if (drag.moving) {
    justDragged = true;
    island.classList.remove('dragging');
    invoke('save_position', position);
  }
  drag = null;
  updateHidden();
}
island.addEventListener('pointerup', endDrag);
island.addEventListener('pointercancel', endDrag);
window.addEventListener('resize', () => place(position));

island.addEventListener('click', (event) => {
  if (justDragged) {
    justDragged = false;
    return;
  }
  const action = event.target.closest('[data-action]')?.dataset.action;
  if (action === 'allow' || action === 'deny') {
    if (ui.approval !== null) invoke('ui_action', { action, id: ui.approval });
    return;
  }
  if (action === 'settings') {
    invoke('open_settings');
    return;
  }
  if (action) {
    invoke('ui_action', { action });
    return;
  }
  pokeUntil = seconds() + 0.6;
  if (ui.paused || ['idle', 'sleep', 'done', 'error'].includes(ui.mode)) invoke('ui_action', { action: 'listen' });
});

island.addEventListener('contextmenu', (event) => {
  event.preventDefault();
  invoke('ui_action', { action: 'pause' });
});

island.addEventListener('mousemove', (event) => {
  const r = canvas.getBoundingClientRect();
  const center = r.left + r.width / 2;
  pointerLook = event.clientX < center - 24 ? -1 : event.clientX > center + 24 ? 1 : 0;
});
island.addEventListener('mouseleave', () => {
  pointerLook = null;
});

new ResizeObserver(reportRegion).observe(island);
place(position);
updateHidden();
invoke('notch_position').then((saved) => saved && place(saved));
tauri.event.listen('position', (event) => place(event.payload));
// The pointer crossing the pill's input region, from GTK: WebKit misses both directions here.
tauri.event.listen('pointer', (event) => {
  if (event.payload === true) {
    pointerIn();
  } else {
    pointerLook = null;
    pointerLeft();
  }
});

tauri.event.listen('notch', (event) => render(event.payload));
tauri.event.listen('sfx', (event) => play(event.payload));
tauri.event.listen('level', (event) => showLevel(Number(event.payload) || 0));
invoke('current_ui').then((current) => current && render(current));
requestAnimationFrame(tick);
