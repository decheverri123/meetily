import { describe, expect, test } from 'bun:test';
import {
  buildTimestampedTranscript,
  citedSegmentIds,
  findSegmentAtTime,
  parseAnswerCitations,
} from '../../src/lib/askCitations';
import type { Transcript, TranscriptSegmentData } from '../../src/types';

const SEGMENTS: TranscriptSegmentData[] = [
  { id: 'a', timestamp: 0, endTime: 4, text: 'Opening remarks.' },
  { id: 'b', timestamp: 24, endTime: 31, text: 'Internal teams go first.' },
  { id: 'c', timestamp: 760, endTime: 768, text: 'Compliance is still open.' },
];

describe('buildTimestampedTranscript', () => {
  test('stamps every line so the model has something to cite', () => {
    const transcripts = [
      { id: '1', text: 'Hello.', timestamp: '', audio_start_time: 5 },
      { id: '2', text: 'Later on.', timestamp: '', audio_start_time: 605.9 },
    ] as Transcript[];

    expect(buildTimestampedTranscript(transcripts)).toBe('[00:05] Hello.\n[10:05] Later on.');
  });

  test('falls back to 00:00 for segments recorded without an audio start time', () => {
    const transcripts = [{ id: '1', text: 'No timing.', timestamp: '' }] as Transcript[];

    expect(buildTimestampedTranscript(transcripts)).toBe('[00:00] No timing.');
  });
});

describe('parseAnswerCitations', () => {
  test('splits an answer into text runs and citation tokens', () => {
    expect(parseAnswerCitations('Internal first [00:24], then beta.')).toEqual([
      { kind: 'text', text: 'Internal first ' },
      { kind: 'citation', label: '00:24', seconds: 24 },
      { kind: 'text', text: ', then beta.' },
    ]);
  });

  test('keeps consecutive citations as separate chips', () => {
    const kinds = parseAnswerCitations('Both said so [00:24][12:40].').map(t => t.kind);
    expect(kinds).toEqual(['text', 'citation', 'citation', 'text']);
  });

  test('reads HH:MM:SS and unbounded minutes', () => {
    expect(parseAnswerCitations('[1:02:03]')).toEqual([
      { kind: 'citation', label: '1:02:03', seconds: 3723 },
    ]);
    expect(parseAnswerCitations('[90:12]')).toEqual([
      { kind: 'citation', label: '90:12', seconds: 5412 },
    ]);
  });

  test('leaves bracketed text that is not a timestamp alone', () => {
    // [Silence] is a real transcript placeholder, and a bare [12] is not a
    // timestamp - neither should turn into a chip.
    expect(parseAnswerCitations('They paused [Silence] at step [12].')).toEqual([
      { kind: 'text', text: 'They paused [Silence] at step [12].' },
    ]);
  });

  test('an answer with no citations is a single text token', () => {
    expect(parseAnswerCitations('Nothing was decided.')).toEqual([
      { kind: 'text', text: 'Nothing was decided.' },
    ]);
  });
});

describe('findSegmentAtTime', () => {
  test('matches a segment by its own start time', () => {
    expect(findSegmentAtTime(SEGMENTS, 24)?.id).toBe('b');
  });

  test('matches a time inside a segment rather than the one before it', () => {
    expect(findSegmentAtTime(SEGMENTS, 28)?.id).toBe('b');
  });

  test('falls back to the last segment that had started', () => {
    expect(findSegmentAtTime(SEGMENTS, 100)?.id).toBe('b');
  });

  test('returns null before the first segment', () => {
    expect(findSegmentAtTime(SEGMENTS, -1)).toBeNull();
  });
});

describe('citedSegmentIds', () => {
  test('de-duplicates and returns ids in transcript order', () => {
    const answer = 'Compliance [12:40] came after the rollout call [00:24], and again [12:44].';

    expect(citedSegmentIds(answer, SEGMENTS)).toEqual(['b', 'c']);
  });

  test('drops citations that resolve to no segment', () => {
    expect(citedSegmentIds('Cited [99:00] and [00:24].', SEGMENTS)).toEqual(['b', 'c']);
    expect(citedSegmentIds('Only [00:24].', [])).toEqual([]);
  });
});
