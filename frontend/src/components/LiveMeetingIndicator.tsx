'use client';

import { usePathname, useRouter } from 'next/navigation';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';

const WAVEFORM_BARS = [0, 1, 2];

/**
 * Persistent "a meeting is recording" affordance, visible from any screen.
 * Home is otherwise the only way back to the live screen and its label
 * doesn't hint that a recording is in progress - this makes the escape
 * route obvious without requiring a trip through Home first.
 */
export function LiveMeetingIndicator() {
  const router = useRouter();
  const pathname = usePathname();
  const { isRecording } = useRecordingState();
  const { meetingTitle } = useTranscripts();

  if (!isRecording || pathname === '/') return null;

  return (
    <button
      type="button"
      onClick={() => router.push('/')}
      title={`Recording: ${meetingTitle} - click to return`}
      className="glass-pill fixed right-6 top-4 z-40 flex items-center gap-2 border-destructive/30 bg-destructive/10 px-3 py-1.5 text-xs font-medium text-destructive shadow-[0_0_20px_-4px_hsl(var(--destructive)/0.5)] transition-colors hover:bg-destructive/20"
    >
      <span className="flex items-center gap-1">
        {WAVEFORM_BARS.map(index => (
          <span
            key={index}
            className="h-2 w-1 rounded-full bg-destructive animate-waveform-bar"
            style={{ animationDelay: `${index * 150}ms` }}
          />
        ))}
      </span>
      Recording
    </button>
  );
}
