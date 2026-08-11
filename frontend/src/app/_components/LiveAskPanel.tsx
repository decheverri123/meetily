'use client';

import { useCallback, useMemo } from 'react';
import { Loader2, Sparkles } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useAskAI } from '@/hooks/useAskAI';
import { useTranscripts } from '@/contexts/TranscriptContext';

/**
 * Free-form Q&A over the meeting currently being recorded. The live-screen
 * sibling of MeetingDetails' AskMeetingPanel, calling
 * `ask_about_live_transcript` instead of `ask_about_meeting`: mid-recording
 * there is no meeting row in SQLite yet, so the transcript-so-far is sent
 * along with the question rather than looked up backend-side by meeting id.
 */
export function LiveAskPanel() {
  const { transcripts } = useTranscripts();

  // The transcript grows for the whole meeting and this component re-renders
  // on every keystroke, so the join is memoized against the segments alone.
  const transcriptText = useMemo(
    () => transcripts.map(t => t.text).join('\n').trim(),
    [transcripts]
  );

  const buildArgs = useCallback(
    (question: string) => ({ transcript: transcriptText, question }),
    [transcriptText]
  );
  const {
    question,
    setQuestion,
    answer,
    isLoading,
    error,
    ask,
    handleKeyDown,
    isSubmitDisabled,
  } = useAskAI('ask_about_live_transcript', buildArgs);

  // Disabled up front rather than round-tripping only to surface the
  // backend's "no transcript yet" rejection.
  const hasTranscript = transcriptText.length > 0;

  return (
    <div className="p-3 space-y-2 glass-panel">
      <div className="flex items-center gap-2 text-sm font-medium text-foreground">
        <Sparkles className="w-4 h-4 text-primary" />
        Ask about this meeting
      </div>
      <div className="flex items-center gap-2">
        <Input
          placeholder="Ask about the meeting so far..."
          value={question}
          onChange={e => setQuestion(e.target.value)}
          onKeyDown={handleKeyDown}
          disabled={isLoading || !hasTranscript}
        />
        <Button
          onClick={ask}
          disabled={isSubmitDisabled || !hasTranscript}
          size="sm"
          aria-label="Ask"
        >
          {isLoading ? <Loader2 className="w-4 h-4 animate-spin" /> : 'Ask'}
        </Button>
      </div>
      {!hasTranscript && (
        <p className="text-xs text-muted-foreground">
          Waiting for the first words of the meeting...
        </p>
      )}
      {error && (
        <p
          className="text-xs text-amber-400 bg-amber-500/10 border border-amber-500/20 rounded-md px-3 py-2"
          aria-live="polite"
        >
          {error}
        </p>
      )}
      {answer && (
        <div
          className="text-sm text-foreground/80 bg-secondary/5 border border-border/10 rounded-md px-3 py-2 whitespace-pre-wrap"
          aria-live="polite"
        >
          {answer}
        </div>
      )}
    </div>
  );
}
