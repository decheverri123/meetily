'use client';

import { useCallback } from 'react';
import { Loader2, Sparkles } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { useAskAI } from '@/hooks/useAskAI';

/**
 * Free-form Q&A across all meetings, shown in the sidebar next to meeting
 * search. Calls the single-shot `ask_across_meetings` Tauri command (not
 * streamed/polled) and renders the result.
 */
export function GlobalAskPanel() {
  const buildArgs = useCallback((question: string) => ({ question }), []);
  const {
    question,
    setQuestion,
    answer,
    isLoading,
    error,
    ask,
    handleKeyDown,
    isSubmitDisabled,
  } = useAskAI('ask_across_meetings', buildArgs);

  return (
    <div className="mt-2 space-y-2">
      <div className="flex items-center gap-2">
        <Input
          placeholder="Ask across all meetings..."
          value={question}
          onChange={e => setQuestion(e.target.value)}
          onKeyDown={handleKeyDown}
          disabled={isLoading}
          className="h-8 text-xs"
        />
        <Button onClick={ask} disabled={isSubmitDisabled} size="sm" variant="outline" aria-label="Ask">
          {isLoading ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Sparkles className="w-3.5 h-3.5" />}
        </Button>
      </div>
      {error && (
        <p
          className="text-xs text-amber-400 bg-amber-500/10 border border-amber-500/20 rounded-md px-2 py-1.5"
          aria-live="polite"
        >
          {error}
        </p>
      )}
      {answer && (
        <div
          className="text-xs text-foreground/80 bg-secondary/5 border border-border/10 rounded-md px-2 py-1.5 whitespace-pre-wrap max-h-40 overflow-y-auto"
          aria-live="polite"
        >
          {answer}
        </div>
      )}
    </div>
  );
}
