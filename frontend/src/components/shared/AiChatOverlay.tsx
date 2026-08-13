'use client';

import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { ArrowDown, MessageSquareText, Sparkles, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { AskSidebar } from './AskSidebar';
import type { TranscriptSegmentData } from '@/types';
import { useAskAI } from '@/hooks/useAskAI';

interface AiChatOverlayProps {
  /** Controlled open state. Defaults to internal state. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Shows a dot on the FAB when collapsed - call site flags on new answers. */
  hasUnread?: boolean;
  /** Tauri command for the "all meetings" ask mode (used when no meeting context). */
  globalCommand?: string;
  /** When provided, mounts the per-meeting AskSidebar instead of the global panel. */
  scoped?: {
    command: string;
    buildArgs: (question: string) => Record<string, unknown>;
    segments: TranscriptSegmentData[];
    suggestions: readonly string[];
    placeholder: string;
    modelLabel: string;
    disabled?: boolean;
    disabledHint?: string;
    headerExtra?: ReactNode;
    /** Fires each time an answer finishes so callers can flag the FAB unread. */
    onAnswered?: () => void;
    /** Lets AskSidebar highlight transcript segments cited by the latest answer. */
    onCitedSegmentsChange?: (segmentIds: string[]) => void;
    onFocusSegment?: (segmentId: string) => void;
  };
  /** FAB label shown to assistive tech while collapsed. */
  fabLabel?: string;
  /** Hide the FAB entirely (e.g. during post-stop finalization). */
  hidden?: boolean;
}

/**
 * Notion-style AI chat: a single circular FAB anchored bottom-right, clicking
 * slides in a full-height chat panel that overlays the content area. Hosts
 * either a scoped (per-meeting) AskSidebar or a global cross-meetings panel,
 * decided by whether `scoped` is supplied.
 *
 * State can be controlled (`open`/`onOpenChange`) for cases where the parent
 * owns the toggle (e.g. Cmd/Ctrl+J). When uncontrolled, internal state drives
 * it.
 */
export function AiChatOverlay({
  open: controlledOpen,
  onOpenChange,
  hasUnread = false,
  globalCommand = 'ask_across_meetings',
  scoped,
  fabLabel = 'Ask AI',
  hidden = false,
}: AiChatOverlayProps) {
  const [internalOpen, setInternalOpen] = useState(false);
  const isControlled = controlledOpen !== undefined;
  const open = isControlled ? controlledOpen : internalOpen;

  const setOpen = useCallback(
    (next: boolean | ((prev: boolean) => boolean)) => {
      const resolved = typeof next === 'function' ? next(open) : next;
      if (!isControlled) setInternalOpen(resolved);
      onOpenChange?.(resolved);
    },
    [isControlled, onOpenChange, open]
  );

  // Dismiss on Escape so a keyboard-only user can close without hunting for X.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, setOpen]);

  if (hidden) return null;

  return (
    <>
      {/* Full-height chat overlay. Animates width 0 -> 420px so the FAB
          doesn't jump. The inner panel crossfades so chat thread stays
          mounted and survives open/close. */}
      <div
        className={cn(
          'fixed right-3 top-3 bottom-3 z-30 flex pointer-events-none',
          'transition-[width] duration-300 ease-out motion-reduce:transition-none',
          open ? 'w-[420px]' : 'w-0'
        )}
        aria-hidden={!open}
      >
        <div
          className={cn(
            'glass-panel flex h-full w-full flex-col overflow-hidden bg-secondary/[.07] shadow-[-30px_0_70px_-30px_rgba(0,0,0,.8)]',
            'transition-opacity duration-200 motion-reduce:transition-none',
            open ? 'pointer-events-auto opacity-100 delay-100' : 'opacity-0'
          )}
        >
          {scoped ? (
            <AskSidebar
              command={scoped.command}
              buildArgs={scoped.buildArgs}
              segments={scoped.segments}
              placeholder={scoped.placeholder}
              suggestions={scoped.suggestions}
              modelLabel={scoped.modelLabel}
              headerExtra={scoped.headerExtra}
              disabled={scoped.disabled}
              disabledHint={scoped.disabledHint}
              onClose={() => setOpen(false)}
              onAnswered={scoped.onAnswered}
              onCitedSegmentsChange={scoped.onCitedSegmentsChange}
              onFocusSegment={scoped.onFocusSegment}
            />
          ) : (
            <GlobalAskContent onClose={() => setOpen(false)} command={globalCommand} />
          )}
        </div>
      </div>

      {/* FAB. Anchored bottom-right; transitions to a close-X inside the panel
          via opacity/scale so users can drop the panel without leaving the
          content area. */}
      <button
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-label={open ? 'Close AI chat' : fabLabel}
        aria-expanded={open}
        className={cn(
          'fixed bottom-6 right-6 z-40 flex h-14 w-14 items-center justify-center rounded-full bg-gradient-to-br from-accent-violet to-primary text-primary-foreground shadow-[0_10px_30px_-8px_rgba(0,0,0,.6)]',
          'transition-all duration-300 ease-out motion-reduce:transition-none',
          'hover:scale-105 active:scale-95',
          open && 'pointer-events-none scale-0 opacity-0'
        )}
      >
        <Sparkles className="h-5 w-5" />
        {hasUnread && !open && (
          <span className="absolute -right-0.5 -top-0.5 h-3.5 w-3.5 rounded-full border-2 border-background bg-destructive" />
        )}
      </button>
    </>
  );
}

/**
 * Inline "ask across all meetings" panel used when no per-meeting context is
 * available. Hosts its own thread via `useAskAI`; conversation stays in-memory
 * for this session only.
 */
function GlobalAskContent({
  command,
  onClose,
}: {
  command: string;
  onClose: () => void;
}) {
  const buildArgs = useCallback((question: string) => ({ question }), []);
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
  } = useAskAI(command, buildArgs);

  return (
    <>
      <div className="flex shrink-0 items-center gap-2.5 border-b border-border/10 px-5 py-4">
        <Sparkles className="h-4 w-4 text-accent-violet" />
        <span className="text-sm font-semibold text-foreground">Ask across all meetings</span>
        <button
          type="button"
          onClick={onClose}
          aria-label="Close AI chat"
          className="ml-auto text-muted-foreground transition-colors hover:text-foreground"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
        {turns.map(turn => (
          <div key={turn.id} className="flex flex-col gap-3">
            <div className="self-end max-w-[84%] rounded-[14px] rounded-br-[4px] border border-border/[.12] bg-secondary/10 px-3.5 py-2.5">
              <p className="text-sm leading-snug text-foreground">{turn.question}</p>
            </div>
            <p className="whitespace-pre-wrap text-sm leading-relaxed text-foreground/85">{turn.answer}</p>
          </div>
        ))}
        {pendingQuestion && (
          <div className="flex flex-col gap-3" aria-live="polite">
            <div className="self-end max-w-[84%] rounded-[14px] rounded-br-[4px] border border-border/[.12] bg-secondary/10 px-3.5 py-2.5">
              <p className="text-sm leading-snug text-foreground">{pendingQuestion}</p>
            </div>
            <div className="flex items-center gap-1 self-start rounded-[14px] rounded-bl-[4px] border border-border/[.12] bg-secondary/10 px-4 py-3">
              {[0, 1, 2].map(i => (
                <span
                  key={i}
                  className="h-1.5 w-1.5 animate-bounce rounded-full bg-muted-foreground"
                  style={{ animationDelay: `${i * 0.15}s` }}
                />
              ))}
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
        {turns.length === 0 && !pendingQuestion && (
          <div className="flex flex-1 flex-col items-start justify-center gap-2 text-sm text-muted-foreground">
            <MessageSquareText className="h-5 w-5 text-accent-violet" />
            <p>Ask anything across all your saved meetings.</p>
            <p className="text-xs">Try: “What decisions did we make last week?”</p>
          </div>
        )}
      </div>

      <div className="flex shrink-0 flex-col gap-2.5 border-t border-border/10 bg-secondary/[.04] p-4">
        <div className="flex items-end gap-2 rounded-xl border border-border/[.14] bg-background/40 px-1.5 py-1.5">
          <textarea
            rows={1}
            placeholder="Ask across all meetings..."
            value={question}
            onChange={e => setQuestion(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={isLoading}
            className="min-h-0 flex-1 resize-none border-0 bg-transparent px-1.5 py-1 text-sm text-foreground shadow-none outline-none placeholder:text-muted-foreground focus:outline-none focus-visible:ring-0"
          />
          <button
            type="button"
            onClick={() => ask()}
            disabled={isSubmitDisabled}
            aria-label="Ask"
            className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-gradient-to-br from-accent-violet to-primary text-primary-foreground disabled:opacity-40"
          >
            <ArrowDown className="h-4 w-4 -rotate-90" />
          </button>
        </div>
      </div>
    </>
  );
}
