'use client';

import { useCallback, useEffect, useMemo, useRef } from 'react';
import { AskSidebar } from '@/components/shared/AskSidebar';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useTranscriptSegments } from '@/hooks/useTranscriptSegments';
import { buildTimestampedTranscript } from '@/lib/askCitations';
import { useSuggestedQuestions } from '@/hooks/useSuggestedQuestions';
import { useConfig } from '@/contexts/ConfigContext';
import { modelConfigLabel, LiveActionChipModelPicker } from './LiveActionChipModelPicker';
import { LiveActionChips } from './LiveActionChips';
import { LiveProviderIndicator } from './LiveProviderIndicator';
import type { LiveActionChipModelOverride, UseLiveActionChipsResult } from '@/hooks/useLiveActionChips';

/**
 * Ask sidebar for the meeting currently being recorded. Calls
 * `ask_about_live_transcript` rather than `ask_about_meeting`: mid-recording
 * there is no meeting row in SQLite yet, so the transcript-so-far is sent
 * along with the question instead of being looked up backend-side by id.
 */

/**
 * Enough transcript for a suggestion to actually be worth generating. Well
 * under the backend's own "no transcript yet" floor is useless here - two
 * words in, the model has nothing to draw a real question from.
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
  /** Fires each time an answer finishes, for a caller showing this collapsed to flag it. */
  onAnswered?: () => void;
  /** Fires once, the first time real (non-empty) suggestions land. */
  onSuggestionsReady?: () => void;
  /**
   * Recap/Questions-to-ask chips, pinned below the header. Kept mounted at
   * the page level (see `useLiveActionChips`'s own doc) so per-chip state
   * survives this panel collapsing/reopening - passed in rather than owned here.
   */
  liveActionChips: UseLiveActionChipsResult;
  liveActionChipOverride: LiveActionChipModelOverride | null;
  onLiveActionChipOverrideChange: (override: LiveActionChipModelOverride | null) => void;
  isRecording: boolean;
}

export function LiveAskPanel({
  fill,
  onCitedSegmentsChange,
  onFocusSegment,
  onClose,
  onAnswered,
  onSuggestionsReady,
  liveActionChips,
  liveActionChipOverride,
  onLiveActionChipOverrideChange,
  isRecording,
}: LiveAskPanelProps) {
  const { transcripts } = useTranscripts();
  const segments = useTranscriptSegments();
  const { modelConfig } = useConfig();

  // The transcript grows for the whole meeting and this component re-renders
  // on every keystroke, so the join is memoized against the segments alone.
  const transcriptText = useMemo(
    () => buildTimestampedTranscript(transcripts),
    [transcripts]
  );

  const { currentMeetingId } = useTranscripts();

  const suggestions = useSuggestedQuestions({
    command: 'suggest_live_transcript_questions',
    args: { transcript: transcriptText },
    scope: currentMeetingId ? `live_${currentMeetingId}` : 'live',
    enabled: transcriptText.length >= MIN_CHARS_FOR_SUGGESTIONS,
    refreshKey: Math.floor(transcriptText.length / SUGGESTION_REFRESH_CHARS),
  });

  const buildArgs = useCallback(
    (question: string) => ({ transcript: transcriptText, question }),
    [transcriptText]
  );

  const hadSuggestionsRef = useRef(false);
  useEffect(() => {
    if (!hadSuggestionsRef.current && suggestions.length > 0) {
      onSuggestionsReady?.();
    }
    hadSuggestionsRef.current = suggestions.length > 0;
  }, [suggestions.length, onSuggestionsReady]);

  // Chips only earn their pinned row once there's something to show - a
  // stopped meeting with no chips generated yet shouldn't reserve the space.
  const showActionChips = isRecording || liveActionChips.hasActivity;

  return (
    <AskSidebar
      command="ask_about_live_transcript"
      buildArgs={buildArgs}
      segments={segments}
      placeholder="Ask about the meeting so far..."
      suggestions={suggestions}
      modelLabel={modelConfigLabel(modelConfig)}
      headerExtra={
        showActionChips && (
          <>
            <LiveActionChips {...liveActionChips} />
            <LiveProviderIndicator provider={liveActionChipOverride?.provider ?? modelConfig.provider} />
            <LiveActionChipModelPicker
              override={liveActionChipOverride}
              onOverrideChange={onLiveActionChipOverrideChange}
            />
          </>
        )
      }
      fill={fill}
      // Disabled up front rather than round-tripping only to surface the
      // backend's "no transcript yet" rejection.
      disabled={transcriptText.length === 0}
      disabledHint="Waiting for the first words of the meeting..."
      onCitedSegmentsChange={onCitedSegmentsChange}
      onFocusSegment={onFocusSegment}
      onClose={onClose}
      onAnswered={onAnswered}
    />
  );
}
