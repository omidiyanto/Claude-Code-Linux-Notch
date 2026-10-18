// The settings window's logic, apart from the page: which file a value comes from, and turning
// the form into what the app saves. Pure functions, unit-tested.

export const MODES = Object.freeze([
  { value: '', title: 'Inherit', desc: "Use the mode set in the other scope, or Claude Code's default." },
  {
    value: 'auto',
    title: 'Auto',
    desc: 'A classifier approves what matches your request and blocks what goes beyond it. Recommended.',
    userOnly: true,
  },
  { value: 'default', title: 'Ask for every action', desc: 'Actions outside the allowlist show up on the notch with Allow / Deny.' },
  { value: 'acceptEdits', title: 'Accept file edits', desc: 'File edits and common filesystem commands run without asking; the rest asks.' },
  { value: 'dontAsk', title: 'Allowlist only', desc: 'Anything not on the allowlist is denied without asking.' },
]);

export const EFFORTS = Object.freeze([
  { value: '', label: 'Inherit' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'xhigh', label: 'X-High' },
]);

export const MODELS = Object.freeze([
  { value: '', label: 'Inherit' },
  { value: 'fable', label: 'Fable' },
  { value: 'opus', label: 'Opus' },
  { value: 'sonnet', label: 'Sonnet' },
  { value: 'haiku', label: 'Haiku' },
  { value: 'custom', label: 'Other…' },
]);

export const SOURCES = Object.freeze({ voice: 'voice only', user: 'all sessions', default: "Claude Code's default" });

/** The value voice commands use for `key` of the managed settings, and which file sets it. */
export function effective(voice, user, key) {
  const fromVoice = voice?.[key] ?? null;
  // Claude Code ignores auto and bypassPermissions in project files such as the voice scope.
  const ignored = key === 'mode' && (fromVoice === 'auto' || fromVoice === 'bypassPermissions');
  if (fromVoice !== null && !ignored) return { value: fromVoice, from: 'voice' };
  const fromUser = user?.[key] ?? null;
  if (fromUser !== null) return { value: fromUser, from: 'user' };
  return { value: null, from: 'default' };
}

/** Trimmed, non-empty lines, each once. */
export function lines(text) {
  return [...new Set(String(text ?? '').split('\n').map((l) => l.trim()).filter(Boolean))];
}

export function modelChoice(model) {
  if (!model) return { select: '', custom: '' };
  const known = MODELS.some((m) => m.value === model && m.value !== 'custom');
  return known ? { select: model, custom: '' } : { select: 'custom', custom: model };
}

/** The form's values as the `Managed` settings the app saves; null means "not set here". */
export function toManaged(form) {
  const model = form.model === 'custom' ? form.custom.trim() : form.model;
  const allow = lines(form.allow);
  return {
    model: model || null,
    effort: form.effort || null,
    mode: form.mode || null,
    allow: allow.length ? allow : null,
  };
}

export function sameManaged(a, b) {
  return JSON.stringify(toComparable(a)) === JSON.stringify(toComparable(b));
}

function toComparable(m) {
  return [m?.model ?? null, m?.effort ?? null, m?.mode ?? null, m?.allow ?? null];
}

/** "just now", "5 min ago", … for a time in seconds since the epoch. */
export function timeAgo(then, now) {
  const s = Math.max(0, Math.floor(now - then));
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  const days = Math.floor(s / 86400);
  return days === 1 ? '1 day ago' : `${days} days ago`;
}
