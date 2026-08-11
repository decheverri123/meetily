'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowUp, Check, Copy, Loader2, PanelRightClose, Sparkles } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { useAskAI } from '@/hooks/useAskAI';
import { citedSegmentIds, parseAnswerCitations } from '@/lib/askCitations';
import type { TranscriptSegmentData } from '@/types';
import { CitationChip } from './CitationChip';
import { cn } from '@/lib/utils';

/**
 * Docked "ask this meeting" conversation, shared by the live recording screen
 * and the saved meeting-details screen. Both ask a single-shot Tauri command
 * over one meeting's transcript and differ only in which command and how the
 * transcript reaches it, so everything below - threading, citation chips,
 * suggestions, composer - lives here once.
 *
 * Answers are cited: the transcript reaches the LLM with `[MM:SS]`-stamped
 * lines, the system prompt asks for those stamps back inline, and each one
 * renders as a chip that highlights (and scrolls to) its transcript segment.
 */
export interface AskSidebarProps {
  /** Tauri command to invoke, e.g. 'ask_about_meeting'. */
  command: string;
  /** Builds the invoke() args from the trimmed question. */
  buildArgs: (question: string) => Record<string, unknown>;
  /** Transcript segments citations are resolved against. */
  segments: TranscriptSegmentData[];
  placeholder: string;
  suggestions: readonly string[];
  /** Footer line stating what the answers are drawn from. */
  scopeNote: string;
  /** Blocks asking when there is nothing to answer from yet. */
  disabled?: boolean;
  disabledHint?: string;
  /** Lets the sidebar grow past its docked width to absorb free space. */
  fill?: boolean;
  /** Segment ids cited by the latest answer, for transcript highlighting. */
  onCitedSegmentsChange?: (segmentIds: string[]) => void;
  /** Segment a citation chip was clicked on, for the transcript to scroll to. */
  onFocusSegment?: (segmentId: string) => void;
  onClose?: () => void;
}

export function AskSidebar({
  command,
  buildArgs,
  segments,
  placeholder,
  suggestions,
  scopeNote,
  disabled = false,
  disabledHint,
  fill = false,
  onCitedSegmentsChange,
  onFocusSegment,
  onClose,
}: AskSidebarProps) {
  const [copiedTurnId, setCopiedTurnId] = useState<string | null>(null);

  const {
    question,
    setQuestion,
    turns,
    pendingQuestion,
    isLoading,
    error,
    ask,
    handleKeyDown,
    isSubmitDisabled,
  } = useAskAI(command, buildArgs, { clearQuestionOnSubmit: true });

  const latestAnswer = turns.length > 0 ? turns[turns.length - 1].answer : null;
  const citedIds = useMemo(
    () => (latestAnswer ? citedSegmentIds(latestAnswer, segments) : []),
    [latestAnswer, segments]
  );

  // Keyed on contents, not array identity: the caller typically feeds this
  // straight into setState, and `segments` re-identifies on every transcript
  // update, so firing per recomputation would loop render -> setState.
  const citedKey = citedIds.join(',');
  const lastCitedKey = useRef<string | null>(null);
  useEffect(() => {
    if (lastCitedKey.current === citedKey) return;
    lastCitedKey.current = citedKey;
    onCitedSegmentsChange?.(citedIds);
  }, [citedKey, citedIds, onCitedSegmentsChange]);

  const copyAnswer = useCallback((turnId: string, answer: string) => {
    void navigator.clipboard?.writeText(answer);
    setCopiedTurnId(turnId);
  }, []);

  // Reset the transient "Copied" confirmation without leaving a timer running
  // past unmount or past the next copy.
  useEffect(() => {
    if (!copiedTurnId) return;
    const timer = setTimeout(() => setCopiedTurnId(null), 1500);
    return () => clearTimeout(timer);
  }, [copiedTurnId]);

  return (
    <aside
      className={cn(
        'glass-panel flex h-full w-[400px] shrink-0 flex-col overflow-hidden bg-secondary/[.07] shadow-[-30px_0_70px_-30px_rgba(0,0,0,.8)]',
        fill && 'flex-1 max-w-[720px]'
      )}
    >
      <div className="flex shrink-0 items-center gap-2.5 border-b border-border/10 px-5 py-4">
        <Sparkles className="h-4 w-4 text-accent-violet" />
        <span className="text-sm font-semibold text-foreground">Ask this meeting</span>
        {onClose && (
          <button
            type="button"
            onClick={onClose}
            aria-label="Close ask panel"
            title="Close ask panel (⌘J)"
            className="ml-auto text-muted-foreground transition-colors hover:text-foreground"
          >
            <PanelRightClose className="h-4 w-4" />
          </button>
        )}
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
        {turns.map(turn => (
          <div key={turn.id} className="flex flex-col gap-3">
            <QuestionBubble question={turn.question} />
            <p className="text-sm leading-relaxed text-foreground/85">
              {parseAnswerCitations(turn.answer).map((token, index) =>
                token.kind === 'text' ? (
                  <span key={index}>{token.text}</span>
                ) : (
                  <CitationChip
                    key={index}
                    seconds={token.seconds}
                    segments={segments}
                    onFocusSegment={onFocusSegment}
                  />
                )
              )}
            </p>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => copyAnswer(turn.id, turn.answer)}
                className="inline-flex items-center gap-1.5 rounded-lg border border-border/10 bg-secondary/[.06] px-2.5 py-1.5 text-xs font-medium text-foreground/80 transition-colors hover:bg-secondary/10"
              >
                {copiedTurnId === turn.id ? (
                  <><Check className="h-3 w-3" />Copied</>
                ) : (
                  <><Copy className="h-3 w-3" />Copy</>
                )}
              </button>
            </div>
          </div>
        ))}

        {pendingQuestion && (
          <div className="flex flex-col gap-3">
            <QuestionBubble question={pendingQuestion} />
            <div className="flex items-center gap-2 text-xs text-muted-foreground" aria-live="polite">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              Reading the transcript...
            </div>
          </div>
        )}

        {error && (
          <p
            className="rounded-md border border-destructive/20 bg-destructive/10 px-3 py-2 text-xs text-destructive"
            aria-live="polite"
          >
            {error}
          </p>
        )}

        {disabled && disabledHint && (
          <p className="text-xs text-muted-foreground">{disabledHint}</p>
        )}

        <div className="mt-auto flex flex-col gap-2.5 pt-2">
          <span className="font-mono text-[10px] tracking-[0.12em] text-muted-foreground">
            SUGGESTED
          </span>
          <div className="flex flex-wrap gap-2">
            {suggestions.map(suggestion => (
              <button
                key={suggestion}
                type="button"
                onClick={() => setQuestion(suggestion)}
                disabled={isLoading || disabled}
                className="glass-pill px-3 py-1.5 text-xs text-foreground/75 transition-colors hover:bg-secondary/10 disabled:opacity-50"
              >
                {suggestion}
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="flex shrink-0 flex-col gap-2.5 border-t border-border/10 bg-secondary/[.04] p-4">
        <div className="flex items-center gap-2 rounded-xl border border-border/[.14] bg-background/40 px-1.5 py-1.5">
          <Input
            placeholder={placeholder}
            value={question}
            onChange={e => setQuestion(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={isLoading || disabled}
            className="border-0 bg-transparent shadow-none focus-visible:ring-0"
          />
          <button
            type="button"
            onClick={ask}
            disabled={isSubmitDisabled || disabled}
            aria-label="Ask"
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-gradient-to-br from-accent-violet to-primary text-primary-foreground disabled:opacity-40"
          >
            {isLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : <ArrowUp className="h-4 w-4" />}
          </button>
        </div>
        <div className="flex items-center gap-2 font-mono text-[10.5px] text-muted-foreground">
          <span>{scopeNote}</span>
          <span className="ml-auto rounded border border-border/10 bg-secondary/[.07] px-1.5 py-0.5">⌘J</span>
        </div>
      </div>
    </aside>
  );
}

function QuestionBubble({ question }: { question: string }) {
  return (
    <div className="self-end max-w-[84%] rounded-[14px] rounded-br-[4px] border border-border/[.12] bg-secondary/10 px-3.5 py-2.5">
      <p className="text-sm leading-snug text-foreground">{question}</p>
    </div>
  );
}
