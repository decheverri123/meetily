'use client';

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ArrowUp, Check, Copy, Loader2, PanelRightClose, Sparkles } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Textarea } from '@/components/ui/textarea';
import { useAskAI } from '@/hooks/useAskAI';
import { citedSegmentIds, parseAnswerCitations } from '@/lib/askCitations';
import type { TranscriptSegmentData } from '@/types';
import { CitationChip } from './CitationChip';
import { cn } from '@/lib/utils';

/** Renders inline markdown (bold, code, links, ...) without wrapping the
 * result in a block-level `<p>`, so it can sit inline with citation chips
 * inside the answer's own `<p>`. */
function InlineMarkdown({ text }: { text: string }) {
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ p: ({ children }) => <>{children}</> }}>
      {text}
    </ReactMarkdown>
  );
}

/** Three dots bouncing in sequence, iMessage-style, shown while an answer is generating. */
function TypingBubble() {
  return (
    <div className="flex items-center gap-1 self-start rounded-[14px] rounded-bl-[4px] border border-border/[.12] bg-secondary/10 px-4 py-3">
      {[0, 1, 2].map(i => (
        <span
          key={i}
          className="h-1.5 w-1.5 animate-bounce rounded-full bg-muted-foreground"
          style={{ animationDelay: `${i * 0.15}s` }}
        />
      ))}
    </div>
  );
}

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
/** Composer stops growing past this height and scrolls internally instead. */
const COMPOSER_MAX_HEIGHT = 160;

export interface AskSidebarProps {
  /** Tauri command to invoke, e.g. 'ask_about_meeting'. */
  command: string;
  /** Builds the invoke() args from the trimmed question. */
  buildArgs: (question: string) => Record<string, unknown>;
  /** Transcript segments citations are resolved against. */
  segments: TranscriptSegmentData[];
  placeholder: string;
  suggestions: readonly string[];
  /** Footer line naming the model answers are generated with. */
  modelLabel: string;
  /** Pinned row below the header, outside the scrollable thread - e.g. quick-action chips. */
  headerExtra?: ReactNode;
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
  /** Fires each time an answer finishes, for a caller showing this collapsed to flag it. */
  onAnswered?: () => void;
}

export function AskSidebar({
  command,
  buildArgs,
  segments,
  placeholder,
  suggestions,
  modelLabel,
  headerExtra,
  disabled = false,
  disabledHint,
  fill = false,
  onCitedSegmentsChange,
  onFocusSegment,
  onClose,
  onAnswered,
}: AskSidebarProps) {
  const [copiedTurnId, setCopiedTurnId] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

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

  // Fires on every new turn, not just while collapsed - it's cheap, and the
  // caller (which knows its own open/collapsed state) decides what to do
  // with it rather than this component tracking visibility itself.
  const turnCountRef = useRef(turns.length);
  useEffect(() => {
    if (turns.length > turnCountRef.current) {
      onAnswered?.();
    }
    turnCountRef.current = turns.length;
  }, [turns.length, onAnswered]);

  // Grows the composer with its content instead of scrolling a single line
  // out of view. Reset to 'auto' first so shrinking (e.g. after submit)
  // isn't stuck at the tallest height it ever reached.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_HEIGHT)}px`;
  }, [question]);

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

      {headerExtra && (
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border/10 px-5 py-3">
          {headerExtra}
        </div>
      )}

      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
        {turns.map(turn => (
          <div key={turn.id} className="flex flex-col gap-3">
            <QuestionBubble question={turn.question} />
            <p className="text-sm leading-relaxed text-foreground/85">
              {parseAnswerCitations(turn.answer).map((token, index) =>
                token.kind === 'text' ? (
                  <InlineMarkdown key={index} text={token.text} />
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
          <div className="flex flex-col gap-3" aria-live="polite" aria-label="Generating answer">
            <QuestionBubble question={pendingQuestion} />
            <TypingBubble />
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

        {suggestions.length > 0 && (
          <div className="mt-auto flex flex-col gap-2.5 pt-2">
            <span className="font-mono text-[10px] tracking-[0.12em] text-muted-foreground">
              SUGGESTED
            </span>
            <div className="flex flex-wrap gap-2">
              {suggestions.map((suggestion, index) => (
                <button
                  key={suggestion}
                  type="button"
                  onClick={() => ask(suggestion)}
                  disabled={isLoading || disabled}
                  style={{ animationDelay: `${index * 120}ms` }}
                  className="magic-cue-chip glass-pill px-3 py-1.5 text-xs text-foreground/90 transition-all hover:bg-secondary/15 hover:scale-[1.03] active:scale-[0.98] disabled:opacity-50"
                >
                  {suggestion}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="flex shrink-0 flex-col gap-2.5 border-t border-border/10 bg-secondary/[.04] p-4">
        <div className="flex items-end gap-2 rounded-xl border border-border/[.14] bg-background/40 px-1.5 py-1.5">
          <Textarea
            ref={textareaRef}
            rows={1}
            placeholder={placeholder}
            value={question}
            onChange={e => setQuestion(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={isLoading || disabled}
            className="min-h-0 resize-none border-0 bg-transparent px-1.5 py-1 shadow-none focus-visible:ring-0"
            style={{ maxHeight: COMPOSER_MAX_HEIGHT }}
          />
          <button
            type="button"
            onClick={() => ask()}
            disabled={isSubmitDisabled || disabled}
            aria-label="Ask"
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-gradient-to-br from-accent-violet to-primary text-primary-foreground disabled:opacity-40"
          >
            {isLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : <ArrowUp className="h-4 w-4" />}
          </button>
        </div>
        <div className="flex items-center gap-2 font-mono text-[10.5px] text-muted-foreground">
          <span>{modelLabel}</span>
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
