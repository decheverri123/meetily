/**
 * Recording-relative timestamp formatting/parsing, shared by the transcript
 * view (which labels every segment) and the ask panels (which cite segments
 * back by timestamp). Both sides must agree on the exact `[MM:SS]` shape or
 * citations parsed out of an LLM answer won't match any segment.
 */

/** Formats seconds from recording start as `[MM:SS]`, e.g. `[07:32]`. */
export function formatRecordingTime(seconds: number | undefined): string {
  if (seconds === undefined) return '[--:--]';

  const totalSeconds = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(totalSeconds / 60);
  const secs = totalSeconds % 60;

  return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

/** Same as {@link formatRecordingTime} without the surrounding brackets. */
export function formatRecordingTimeLabel(seconds: number | undefined): string {
  return formatRecordingTime(seconds).slice(1, -1);
}

/**
 * Parses `MM:SS` or `HH:MM:SS` (as emitted in LLM citations) into seconds.
 * Minutes are unbounded so a 90-minute meeting can cite `[90:12]` rather than
 * rolling over to an hour field. Returns null for anything unparseable.
 */
export function parseRecordingTime(label: string): number | null {
  const parts = label.trim().split(':');
  if (parts.length < 2 || parts.length > 3) return null;

  const numbers = parts.map(part => (/^\d{1,3}$/.test(part) ? Number(part) : NaN));
  if (numbers.some(Number.isNaN)) return null;

  const [a, b, c] = numbers;
  return parts.length === 2 ? a * 60 + b : a * 3600 + b * 60 + c;
}
