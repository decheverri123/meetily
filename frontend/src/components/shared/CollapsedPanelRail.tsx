'use client';

import { PanelLeftOpen } from 'lucide-react';
import { cn } from '@/lib/utils';

/**
 * The narrow rail a collapsed side panel shows, on both the live and
 * meeting-details screens. Rendered as an overlay inside the panel's own
 * container and crossfaded rather than swapped in: the panel underneath stays
 * mounted through the collapse so its scroll position (and, on the live
 * screen, the virtualizer's measurements) survive the round trip.
 */
export function CollapsedPanelRail({
  label,
  meta,
  visible,
  onExpand,
  expandTitle,
}: {
  label: string;
  /** Short secondary line, e.g. a segment count. */
  meta?: string;
  visible: boolean;
  onExpand: () => void;
  expandTitle: string;
}) {
  return (
    <div
      aria-hidden={!visible}
      className={cn(
        'absolute inset-0 z-10 flex flex-col items-center gap-4 py-3 transition-opacity duration-200 motion-reduce:transition-none',
        visible ? 'opacity-100 delay-100' : 'pointer-events-none opacity-0'
      )}
    >
      <button
        type="button"
        onClick={onExpand}
        title={expandTitle}
        aria-label={expandTitle}
        tabIndex={visible ? 0 : -1}
        className="text-muted-foreground transition-colors hover:text-foreground"
      >
        <PanelLeftOpen className="h-4 w-4" />
      </button>
      <div className="flex items-center gap-3 [writing-mode:vertical-rl]">
        <span className="whitespace-nowrap text-[11px] font-semibold uppercase tracking-[0.14em] text-foreground/80">
          {label}
        </span>
        {meta && <span className="font-mono text-[10px] text-muted-foreground">{meta}</span>}
      </div>
    </div>
  );
}
