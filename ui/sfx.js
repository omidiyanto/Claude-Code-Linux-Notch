// Tiny synthesized chirps, so the buddy needs no sound files. Each sound is [frequency Hz, seconds]
// notes in a row; frequency 0 is a rest.

export const SOUNDS = Object.freeze({
  wake: [[660, 0.06], [990, 0.09]],
  done: [[523, 0.06], [659, 0.06], [784, 0.12]],
  error: [[392, 0.09], [262, 0.16]],
  attention: [[880, 0.07], [0, 0.05], [880, 0.07]],
  cancel: [[587, 0.05], [392, 0.08]],
});

const VOLUME = 0.08;
let context = null;

export function play(name) {
  const notes = SOUNDS[name];
  if (!notes) return;
  try {
    context ??= new AudioContext();
    let at = context.currentTime + 0.01;
    for (const [freq, seconds] of notes) {
      if (freq > 0) {
        const osc = context.createOscillator();
        const gain = context.createGain();
        osc.type = 'triangle';
        osc.frequency.value = freq;
        gain.gain.setValueAtTime(0.0001, at);
        gain.gain.exponentialRampToValueAtTime(VOLUME, at + 0.01);
        gain.gain.exponentialRampToValueAtTime(0.0001, at + seconds);
        osc.connect(gain).connect(context.destination);
        osc.start(at);
        osc.stop(at + seconds + 0.02);
      }
      at += seconds;
    }
  } catch (e) {
    // No audio device or no Web Audio in this WebKit build: stay silent.
    console.warn('sound', e);
  }
}
