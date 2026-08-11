'use client';

import { useCallback } from 'react';
import { AskSidebar } from '@/components/shared/AskSidebar';
import type { TranscriptSegmentData } from '@/types';
import { useSuggestedQuestions } from '@/hooks/useSuggestedQuestions';
import { useConfig } from '@/contexts/ConfigContext';
import { modelConfigLabel } from '@/app/_components/LiveActionChipModelPicker';

/**
 * Ask sidebar for a saved meeting. Calls the single-shot `ask_about_meeting`
 * Tauri command, which builds its own context from the stored summary and
 * transcript - so unlike the live panel, only the meeting id goes over.
 */

interface AskMeetingPanelProps {
  meetingId: string;
  /**
   * Loaded transcript segments, for resolving citations. Paginated screens may
   * hold only part of the meeting; citations outside it render inert.
   */
  segments?: TranscriptSegmentData[];
  onCitedSegmentsChange?: (segmentIds: string[]) => void;
  onFocusSegment?: (segmentId: string) => void;
  onClose?: () => void;
}

const NO_SEGMENTS: TranscriptSegmentData[] = [];

export function AskMeetingPanel({
  meetingId,
  segments = NO_SEGMENTS,
  onCitedSegmentsChange,
  onFocusSegment,
  onClose,
}: AskMeetingPanelProps) {
  const { modelConfig } = useConfig();
  const suggestions = useSuggestedQuestions({
    command: 'suggest_meeting_questions',
    args: { meetingId },
    scope: meetingId,
  });

  const buildArgs = useCallback(
    (question: string) => ({ meetingId, question }),
    [meetingId]
  );

  return (
    <AskSidebar
      command="ask_about_meeting"
      buildArgs={buildArgs}
      segments={segments}
      placeholder="Ask a question about this meeting..."
      suggestions={suggestions}
      modelLabel={modelConfigLabel(modelConfig)}
      onCitedSegmentsChange={onCitedSegmentsChange}
      onFocusSegment={onFocusSegment}
      onClose={onClose}
    />
  );
}
