'use client';

import { PanelLeftOpen } from 'lucide-react';

/**
 * A collapsed side panel, reduced to the same narrow glass rail on both the
 * live and meeting-details screens. Keeps the panel's label readable (rotated
 * up the rail) so the column still says what expanding it brings back.
 */
export function CollapsedPanelRail({
  label,
  meta,
  onExpand,
  expandTitle,
}: {
  label: string;
  /** Short secondary line, e.g. a segment count. */
  meta?: string;
  onExpand: () => void;
  expandTitle: string;
}) {
  return (
    <div className="glass-panel flex w-11 shrink-0 flex-col items-center gap-4 overflow-hidden py-3">
      <button
        type="button"
        onClick={onExpand}
        title={expandTitle}
        aria-label={expandTitle}
        className="text-muted-foreground transition-colors hover:text-foreground"
      >
        <PanelLeftOpen className="h-4 w-4" />
      </button>
      <div className="flex items-center gap-3 [writing-mode:vertical-rl]">
        <span className="text-[11px] font-semibold uppercase tracking-[0.14em] text-foreground/80">
          {label}
        </span>
        {meta && <span className="font-mono text-[10px] text-muted-foreground">{meta}</span>}
      </div>
    </div>
  );
}
