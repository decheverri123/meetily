import { ClipboardList, HelpCircle, Loader2 } from 'lucide-react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { MarkdownContent } from '@/components/MarkdownContent';
import { cn } from '@/lib/utils';
import type { LiveActionChipKind, UseLiveActionChipsResult } from '@/hooks/useLiveActionChips';

const CHIP_CONFIG: Record<LiveActionChipKind, { label: string; icon: typeof ClipboardList }> = {
  recap: { label: 'Recap', icon: ClipboardList },
  questions: { label: 'Questions to ask', icon: HelpCircle },
};

/**
 * "Recap" and "Questions to ask" action chips shown during live recording.
 *
 * Each chip is a small pill button that, on click, invokes
 * `generate_live_action_chip` for its kind and shows the markdown result in
 * a popover anchored to the chip. Purely presentational - `useLiveActionChips()`
 * is called by the parent and kept mounted for the life of the meeting so its
 * per-kind state survives this component re-rendering.
 */
export function LiveActionChips({ chips, generate, isRecording }: UseLiveActionChipsResult) {
  return (
    <div className="flex items-center gap-2">
      {(Object.keys(CHIP_CONFIG) as LiveActionChipKind[]).map(kind => {
        const { label, icon: Icon } = CHIP_CONFIG[kind];
        const { result, isLoading, error, isRetryable, hasGenerated } = chips[kind];

        return (
          <Popover key={kind}>
            <PopoverTrigger asChild>
              <button
                type="button"
                onClick={() => generate(kind)}
                disabled={isLoading || !isRecording}
                className="inline-flex items-center gap-1.5 rounded-full border border-gray-200 bg-white px-3 py-1.5 text-xs font-medium text-gray-700 shadow-sm transition-colors hover:bg-gray-50 disabled:cursor-not-allowed disabled:opacity-70"
                title={label}
              >
                {isLoading ? (
                  <Loader2 className="w-3.5 h-3.5 animate-spin" />
                ) : (
                  <Icon className="w-3.5 h-3.5" />
                )}
                {label}
              </button>
            </PopoverTrigger>
            <PopoverContent className="w-80 max-h-80 overflow-y-auto text-sm" align="center" sideOffset={8}>
              {isLoading && !result && (
                <div className="flex items-center gap-2 text-gray-400 py-4 justify-center">
                  <Loader2 className="w-4 h-4 animate-spin" />
                  <span>Generating...</span>
                </div>
              )}

              {error && (
                <div
                  className={cn(
                    'text-xs rounded-md px-3 py-2',
                    isRetryable
                      ? 'text-gray-500 bg-gray-50 border border-gray-200'
                      : 'text-amber-600 bg-amber-50 border border-amber-200'
                  )}
                >
                  {error}
                </div>
              )}

              {!isLoading && !error && !result && (
                <p className="text-gray-400 text-center py-4">
                  {hasGenerated
                    ? 'Not enough conversation yet — keep talking and try again.'
                    : `Click “${label}” to generate.`}
                </p>
              )}

              {result && <MarkdownContent>{result}</MarkdownContent>}
            </PopoverContent>
          </Popover>
        );
      })}
    </div>
  );
}
