'use client';

import { MessageSquareText } from 'lucide-react';
import { cn } from '@/lib/utils';

interface AskFloatingBubbleProps {
  open: boolean;
  /** Shows a dot on the launcher - a suggestion landed or an answer finished while collapsed. */
  hasUnread: boolean;
  onOpen: () => void;
  children: React.ReactNode;
}

/**
 * Floating "ask this meeting" launcher for the live recording screen. The
 * panel (`children`) is always mounted - only its opacity/scale/pointer-events
 * toggle - so its conversation thread and scroll position survive being
 * collapsed and reopened, same rationale as `CollapsedPanelRail`.
 */
export function AskFloatingBubble({ open, hasUnread, onOpen, children }: AskFloatingBubbleProps) {
  return (
    <div className="fixed bottom-28 right-6 z-30 flex flex-col items-end gap-3">
      <div
        className={cn(
          'h-[min(70vh,640px)] origin-bottom-right transition-all duration-300 ease-out motion-reduce:transition-none',
          open ? 'scale-100 opacity-100' : 'pointer-events-none translate-y-3 scale-90 opacity-0'
        )}
      >
        {children}
      </div>

      <button
        type="button"
        onClick={onOpen}
        aria-label="Ask this meeting"
        className={cn(
          'relative flex h-14 w-14 items-center justify-center rounded-full bg-gradient-to-br from-accent-violet to-primary text-primary-foreground shadow-[0_10px_30px_-8px_rgba(0,0,0,.6)] transition-all duration-300 ease-out hover:scale-105 motion-reduce:transition-none',
          open && 'pointer-events-none scale-0 opacity-0'
        )}
      >
        <MessageSquareText className="h-5 w-5" />
        {hasUnread && (
          <span className="absolute -right-0.5 -top-0.5 h-3.5 w-3.5 rounded-full border-2 border-background bg-destructive" />
        )}
      </button>
    </div>
  );
}
