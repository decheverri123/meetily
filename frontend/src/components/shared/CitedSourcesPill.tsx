'use client';

import { Quote } from 'lucide-react';

/** How many transcript segments the ask sidebar's latest answer drew on. */
export function CitedSourcesPill({ count }: { count: number }) {
  if (count === 0) return null;

  return (
    <span className="inline-flex items-center gap-2 rounded-[10px] border border-primary/25 bg-primary/10 px-3 py-1.5 font-mono text-[11px] text-primary">
      <Quote className="h-3 w-3" />
      {count} SOURCE{count === 1 ? '' : 'S'} FOR THIS ANSWER
    </span>
  );
}
