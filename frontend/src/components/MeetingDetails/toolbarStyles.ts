/**
 * Shared chrome for the meeting-details column headers: one prominent primary
 * action plus icon-only tiles for everything secondary.
 */

/** 34px tile, used in the note column header. */
export const headerTileClass =
  'h-[34px] w-[34px] rounded-xl border border-border/10 bg-secondary/10 text-muted-foreground hover:bg-secondary/20 hover:text-foreground';

/** 28px tile, used in the narrower transcript column header. */
export const compactTileClass =
  'h-7 w-7 rounded-lg border border-border/10 bg-secondary/10 text-muted-foreground hover:bg-secondary/20 hover:text-foreground [&_svg]:size-3.5';

/** Shared sizing/shape for the pill-shaped primary action button (Generate/Stop). */
export const primaryActionButtonClass = 'h-[34px] rounded-xl px-3.5 font-semibold';
