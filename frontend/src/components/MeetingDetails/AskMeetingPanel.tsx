'use client';

import { useCallback } from 'react';
import { Loader2, Sparkles } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useAskAI } from '@/hooks/useAskAI';

interface AskMeetingPanelProps {
  meetingId: string;
}

/**
 * Free-form Q&A over the current meeting. Calls the single-shot
 * `ask_about_meeting` Tauri command (not streamed/polled - a plain invoke()
 * that resolves once the LLM answer is ready) and renders the result.
 */
export function AskMeetingPanel({ meetingId }: AskMeetingPanelProps) {
  const buildArgs = useCallback(
    (question: string) => ({ meetingId, question }),
    [meetingId]
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
  } = useAskAI('ask_about_meeting', buildArgs);

  return (
    <div className="p-3 space-y-2 glass-panel">
      <div className="flex items-center gap-2 text-sm font-medium text-foreground">
        <Sparkles className="w-4 h-4 text-primary" />
        Ask about this meeting
      </div>
      <div className="flex items-center gap-2">
        <Input
          placeholder="Ask a question about this meeting..."
          value={question}
          onChange={e => setQuestion(e.target.value)}
          onKeyDown={handleKeyDown}
          disabled={isLoading}
        />
        <Button onClick={ask} disabled={isSubmitDisabled} size="sm" aria-label="Ask">
          {isLoading ? <Loader2 className="w-4 h-4 animate-spin" /> : 'Ask'}
        </Button>
      </div>
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
