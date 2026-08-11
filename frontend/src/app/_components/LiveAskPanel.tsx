'use client';

import { useCallback, useMemo } from 'react';
import { AskSidebar } from '@/components/shared/AskSidebar';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useTranscriptSegments } from '@/hooks/useTranscriptSegments';
import { buildTimestampedTranscript } from '@/lib/askCitations';
import { useSuggestedQuestions } from '@/hooks/useSuggestedQuestions';

/**
 * Ask sidebar for the meeting currently being recorded. Calls
 * `ask_about_live_transcript` rather than `ask_about_meeting`: mid-recording
 * there is no meeting row in SQLite yet, so the transcript-so-far is sent
 * along with the question instead of being looked up backend-side by id.
 */

// Shown until the meeting has enough transcript to suggest something better.
const FALLBACK_QUESTIONS = [
  'What has been decided so far?',
  'Any risks raised?',
  'tl;dr',
] as const;

/**
 * Enough transcript for a suggestion to beat the generic fallbacks. Well under
 * the backend's own "no transcript yet" floor is useless here - two words in,
 * the model can only echo the fallbacks back.
 */
const MIN_CHARS_FOR_SUGGESTIONS = 400;

/**
 * How much further the meeting has to get before suggestions are regenerated.
 * A long meeting drifts well away from what it opened with, and leaving the
 * first batch up for an hour is what makes suggestion chips feel canned.
 */
const SUGGESTION_REFRESH_CHARS = 4000;

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

  const suggestions = useSuggestedQuestions({
    command: 'suggest_live_transcript_questions',
    args: { transcript: transcriptText },
    scope: 'live',
    fallback: FALLBACK_QUESTIONS,
    enabled: transcriptText.length >= MIN_CHARS_FOR_SUGGESTIONS,
    refreshKey: Math.floor(transcriptText.length / SUGGESTION_REFRESH_CHARS),
  });

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
      suggestions={suggestions}
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
