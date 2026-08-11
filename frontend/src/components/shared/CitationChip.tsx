'use client';

import { useMemo } from 'react';
import { findSegmentAtTime } from '@/lib/askCitations';
import { formatRecordingTimeLabel } from '@/lib/transcriptTime';
import type { TranscriptSegmentData } from '@/types';
import { cn } from '@/lib/utils';

/**
 * A `[MM:SS]` citation lifted out of an AI answer, rendered inline as a chip
 * that jumps to the transcript segment it points at. A citation with no
 * matching segment (the model adjusted a stamp, or that part of the transcript
 * isn't loaded) stays visible but inert rather than jumping to a wrong line.
 */
export function CitationChip({
  seconds,
  segments,
  onFocusSegment,
}: {
  seconds: number;
  segments: TranscriptSegmentData[];
  onFocusSegment?: (segmentId: string) => void;
}) {
  const segment = useMemo(
    () => findSegmentAtTime(segments, seconds),
    [seconds, segments]
  );
  // Normalized so a cited "[7:32]" and the transcript's "07:32" read alike.
  const display = formatRecordingTimeLabel(seconds);

  return (
    <button
      type="button"
      disabled={!segment}
      onClick={() => segment && onFocusSegment?.(segment.id)}
      title={segment ? `Jump to ${display} in the transcript` : `No transcript segment at ${display}`}
      className={cn(
        'mx-0.5 rounded px-1.5 align-super font-mono text-[10.5px]',
        segment ? 'bg-primary/15 text-primary hover:bg-primary/25' : 'bg-secondary/10 text-muted-foreground'
      )}
    >
      {display}
    </button>
  );
}
