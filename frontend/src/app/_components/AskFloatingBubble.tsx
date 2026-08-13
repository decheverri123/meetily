'use client';

import { useEffect, useRef, type MutableRefObject } from 'react';
import { MessageSquareText } from 'lucide-react';
import { cn } from '@/lib/utils';

interface AskFloatingBubbleProps {
  open: boolean;
  /** Shows a dot on the launcher - a suggestion landed or an answer finished while collapsed. */
  hasUnread: boolean;
  onOpen: () => void;
  /** Fires when the user clicks outside the panel or hits Escape. */
  onDismiss?: () => void;
  /**
   * Ref to the composer's input. Auto-focused when the panel opens so keyboard
   * users land on the textarea, not back on the document body.
   */
  composerRef?: MutableRefObject<HTMLTextAreaElement | null>;
  /**
   * Hides the launcher button (e.g. while the meeting is being saved or
   * transcribed - clicking ask during those phases can race the stop handler).
   * Defaults to true.
   */
  enabled?: boolean;
  children: React.ReactNode;
}

/**
 * Floating "ask this meeting" launcher for the live recording screen. The
 * panel (`children`) is always mounted - only its opacity/scale/pointer-events
 * toggle - so its conversation thread and scroll position survive being
 * collapsed and reopened, same rationale as `CollapsedPanelRail`.
 */
export function AskFloatingBubble({
  open,
  hasUnread,
  onOpen,
  onDismiss,
  composerRef,
  enabled = true,
  children,
}: AskFloatingBubbleProps) {
  const panelRef = useRef<HTMLDivElement>(null);

  // Auto-focus composer on open so keyboard users land in a sensible place.
  useEffect(() => {
    if (open) {
      // Defer to the same tick the open animation kicks off; focusing too
      // eagerly can scroll-jump while the panel is still scaling in.
      const id = window.setTimeout(() => {
        composerRef?.current?.focus();
      }, 50);
      return () => window.clearTimeout(id);
    }
    return undefined;
  }, [open, composerRef]);

  // Close on outside click while open. The panel itself has a close button
  // (X) and Cmd/Ctrl+J toggles, but other floating chat UIs also close on
  // outside click - this matches that affordance.
  useEffect(() => {
    if (!open || !onDismiss) return undefined;
    const handlePointer = (event: MouseEvent) => {
      const panel = panelRef.current;
      if (!panel) return;
      // Ignore clicks that originated inside the panel.
      if (event.target instanceof Node && panel.contains(event.target)) return;
      onDismiss();
    };
    // Use mousedown so a drag-select that finishes with mouseup on the
    // launcher doesn't double-toggle.
    document.addEventListener('mousedown', handlePointer);
    return () => document.removeEventListener('mousedown', handlePointer);
  }, [open, onDismiss]);

  // Close on Escape while open - matches other floating chat UIs and the
  // panel's in-card close button affordance.
  useEffect(() => {
    if (!open || !onDismiss) return undefined;
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onDismiss();
    };
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
  }, [open, onDismiss]);

  return (
    <div className="fixed bottom-28 right-4 z-40 flex flex-col items-end gap-3 sm:right-6 md:right-8">
      <div
        ref={panelRef}
        className={cn(
          // Width scales with viewport so the panel doesn't completely cover
          // Live Insights on narrow desktop windows. The meeting-details page
          // uses its own AskSidebar width and ignores this wrapper.
          'w-[min(90vw,400px)] max-w-[400px] h-[min(70vh,640px)] max-h-[calc(100vh-12rem)] origin-bottom-right transition-all duration-300 ease-out motion-reduce:transition-none',
          open ? 'scale-100 opacity-100' : 'pointer-events-none translate-y-3 scale-90 opacity-0'
        )}
      >
        {children}
      </div>

      <button
        type="button"
        onClick={onOpen}
        aria-label="Ask this meeting"
        aria-expanded={open}
        hidden={!enabled}
        className={cn(
          'relative flex h-14 w-14 items-center justify-center rounded-full bg-gradient-to-br from-accent-violet to-primary text-primary-foreground shadow-[0_10px_30px_-8px_rgba(0,0,0,.6)] transition-all duration-300 ease-out hover:scale-105 motion-reduce:transition-none',
          (open || !enabled) && 'pointer-events-none scale-0 opacity-0'
        )}
      >
        <MessageSquareText className="h-5 w-5" />
        {hasUnread && enabled && (
          <span className="absolute -right-0.5 -top-0.5 h-3.5 w-3.5 rounded-full border-2 border-black/40 bg-destructive" />
        )}
      </button>
    </div>
  );
}
