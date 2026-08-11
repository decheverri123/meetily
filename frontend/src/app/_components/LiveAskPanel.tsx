'use client';

import { useCallback, useMemo } from 'react';
import { AskSidebar } from '@/components/shared/AskSidebar';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useTranscriptSegments } from '@/hooks/useTranscriptSegments';
import { buildTimestampedTranscript } from '@/lib/askCitations';

/**
 * Ask sidebar for the meeting currently being recorded. Calls
 * `ask_about_live_transcript` rather than `ask_about_meeting`: mid-recording
 * there is no meeting row in SQLite yet, so the transcript-so-far is sent
 * along with the question instead of being looked up backend-side by id.
 */

const SUGGESTED_QUESTIONS = [
  'What has been decided so far?',
  'Any risks raised?',
  'tl;dr',
] as const;

interface LiveAskPanelProps {
  /** Grows the panel to absorb space freed by a collapsed transcript. */
  fill?: boolean;
  onCitedSegmentsChange?: (segmentIds: string[]) => void;
  onFocusSegment?: (segmentId: string) => void;
  onClose?: () => void;
}

export function LiveAskPanel({
  fill,
  onCitedSegmentsChange,
  onFocusSegment,
  onClose,
}: LiveAskPanelProps) {
  const { transcripts } = useTranscripts();
  const segments = useTranscriptSegments();

  // The transcript grows for the whole meeting and this component re-renders
  // on every keystroke, so the join is memoized against the segments alone.
  const transcriptText = useMemo(
    () => buildTimestampedTranscript(transcripts),
    [transcripts]
  );

  const buildArgs = useCallback(
    (question: string) => ({ transcript: transcriptText, question }),
    [transcriptText]
  );

  return (
    <AskSidebar
      command="ask_about_live_transcript"
      buildArgs={buildArgs}
      segments={segments}
      placeholder="Ask about the meeting so far..."
      suggestions={SUGGESTED_QUESTIONS}
      scopeNote="ANSWERS FROM THIS TRANSCRIPT ONLY"
      fill={fill}
      // Disabled up front rather than round-tripping only to surface the
      // backend's "no transcript yet" rejection.
      disabled={transcriptText.length === 0}
      disabledHint="Waiting for the first words of the meeting..."
      onCitedSegmentsChange={onCitedSegmentsChange}
      onFocusSegment={onFocusSegment}
      onClose={onClose}
    />
  );
}
