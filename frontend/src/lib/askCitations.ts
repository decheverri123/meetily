import { formatRecordingTime, parseRecordingTime } from '@/lib/transcriptTime';
import type { Transcript, TranscriptSegmentData } from '@/types';

/**
 * Citation plumbing for the live ask panel: the transcript is sent to the LLM
 * with every line stamped `[MM:SS]`, the system prompt asks it to cite those
 * stamps inline, and the answer comes back as one plain string. These helpers
 * turn that string back into renderable tokens and resolve each citation to
 * the transcript segment it points at, so the answer and the transcript can
 * highlight together.
 */

/** A stamped line of the transcript context sent to the LLM. */
export function buildTimestampedTranscript(transcripts: Transcript[]): string {
  return transcripts
    .map(t => `${formatRecordingTime(t.audio_start_time ?? 0)} ${t.text}`)
    .join('\n')
    .trim();
}

export type AnswerToken =
  | { kind: 'text'; text: string }
  | { kind: 'citation'; label: string; seconds: number };

/**
 * Matches a bracketed `[MM:SS]` / `[HH:MM:SS]` citation. Kept permissive on
 * digit count (the LLM echoes whatever the transcript showed it) and validated
 * by `parseRecordingTime`, so a bracketed non-timestamp stays literal text.
 */
const CITATION_PATTERN = /\[(\d{1,3}(?::\d{1,2}){1,2})\]/g;

/**
 * Splits an answer into text runs and citation chips. Consecutive citations
 * stay separate tokens so each renders as its own clickable chip.
 */
export function parseAnswerCitations(answer: string): AnswerToken[] {
  const tokens: AnswerToken[] = [];
  let cursor = 0;

  for (const match of answer.matchAll(CITATION_PATTERN)) {
    const seconds = parseRecordingTime(match[1]);
    if (seconds === null) continue;

    const start = match.index!;
    if (start > cursor) {
      tokens.push({ kind: 'text', text: answer.slice(cursor, start) });
    }
    tokens.push({ kind: 'citation', label: match[1], seconds });
    cursor = start + match[0].length;
  }

  if (cursor < answer.length) {
    tokens.push({ kind: 'text', text: answer.slice(cursor) });
  }
  return tokens;
}

/**
 * Resolves a cited second to the segment that was being spoken at that moment:
 * the last segment starting at or before it, preferring one whose end time
 * still covers it. The LLM normally echoes a segment's own start time, so this
 * only has to absorb small drift (a paraphrased or rounded stamp).
 */
export function findSegmentAtTime(
  segments: TranscriptSegmentData[],
  seconds: number
): TranscriptSegmentData | null {
  let best: TranscriptSegmentData | null = null;
  for (const segment of segments) {
    if (segment.timestamp > seconds) break;
    best = segment;
    if (segment.endTime !== undefined && segment.endTime >= seconds) break;
  }
  return best;
}

/** Segment ids cited by an answer, de-duplicated and in transcript order. */
export function citedSegmentIds(
  answer: string,
  segments: TranscriptSegmentData[]
): string[] {
  const ids = new Set<string>();
  for (const token of parseAnswerCitations(answer)) {
    if (token.kind !== 'citation') continue;
    const segment = findSegmentAtTime(segments, token.seconds);
    if (segment) ids.add(segment.id);
  }
  return segments.filter(s => ids.has(s.id)).map(s => s.id);
}
