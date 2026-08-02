// Tiny guarded localStorage wrapper. Private windows, disabled storage and
// blocked third-party contexts all throw on access, and none of them is a
// reason for the dock to stop opening or the volume slider to stop moving:
// every read degrades to null and every write to a no-op.

export const lsGet = (key: string): string | null => {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};

export const lsSet = (key: string, value: string) => {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* ignore */
  }
};

/** A stored number, or `fallback` when it is missing or unparseable.
 *
 * The missing case has to be tested BEFORE the conversion: `Number(null)` and
 * `Number('')` are both 0, and 0 is finite, so the obvious one-liner silently
 * returns 0 for every key that was never written. That is not a hypothetical —
 * it is why a player who had never touched the volume slider had a master
 * gain of zero and heard nothing at all. */
export const lsNum = (key: string, fallback: number): number => {
  const raw = lsGet(key);
  if (raw === null || raw.trim() === '') return fallback;
  const n = Number(raw);
  return Number.isFinite(n) ? n : fallback;
};

/** A stored flag; '1' is true, anything else (including absent) is false. */
export const lsFlag = (key: string): boolean => lsGet(key) === '1';
