'use client';

import { useMemo } from 'react';
import { useTranscripts } from '@/contexts/TranscriptContext';
import type { TranscriptSegmentData } from '@/types';

/**
 * The live transcript in the shape VirtualizedTranscriptView renders and
 * askCitations resolves citations against. Shared by TranscriptPanel and
 * Scoped ask overlay so a cited segment id means the same thing on both sides.
 */
export function useTranscriptSegments(): TranscriptSegmentData[] {
  const { transcripts } = useTranscripts();

  return useMemo(
    () =>
      transcripts.map(t => ({
        id: t.id,
        timestamp: t.audio_start_time ?? 0,
        endTime: t.audio_end_time,
        text: t.text,
        confidence: t.confidence,
      })),
    [transcripts]
  );
}
