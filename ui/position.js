// Where the notch sits. x is the centre as a fraction of the screen width (so it survives a
// resolution change), y the distance from the top in pixels. Pure functions, unit-tested.

export const DEFAULT = Object.freeze({ x: 0.5, y: 0 });
// The largest the pill gets (the approval card): it must always fit on screen.
export const MAX_WIDTH = 460;
export const MAX_HEIGHT = 190;
// Dropped this close to the top, it snaps into the edge and looks like a notch again.
export const SNAP = 16;
// A press that moves less than this is a click, not a drag.
export const DRAG_THRESHOLD = 5;

export function clamp(pos, width, height) {
  if (!(width > 0) || !(height > 0)) return { ...DEFAULT };
  const half = Math.min(MAX_WIDTH / 2, width / 2);
  const x = Number.isFinite(pos.x) ? pos.x : DEFAULT.x;
  const y = Number.isFinite(pos.y) ? pos.y : DEFAULT.y;
  const centre = Math.min(Math.max(x * width, half), width - half);
  const top = y < SNAP ? 0 : Math.min(y, Math.max(0, height - MAX_HEIGHT));
  return { x: centre / width, y: top };
}

/** Position after the pointer moved by (dx, dy) pixels since the drag started at `start`. */
export function dragged(start, dx, dy, width) {
  return { x: start.x + dx / width, y: Math.max(0, start.y + dy) };
}

export function isDrag(dx, dy) {
  return Math.hypot(dx, dy) >= DRAG_THRESHOLD;
}
