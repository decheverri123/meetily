import { Cloud } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { ALWAYS_AVAILABLE_PROVIDERS, providerLabel } from './LiveActionChipModelPicker';

interface LiveProviderIndicatorProps {
  /**
   * Effective provider id for the generation this indicator describes -
   * either the chips' ad-hoc override (`LiveActionChipModelPicker`) or the
   * Settings-configured default (`useConfig().modelConfig.provider`), same
   * as the caller already resolves for `useLiveActionChips`/`useLiveInsights`.
   */
  provider: string;
  className?: string;
}

/**
 * "Cloud (Provider)" badge for the live action chips and Live Insights panel,
 * shown only when generation is actually leaving the machine.
 *
 * Meetily is positioned as privacy-first/local-only, but both features
 * follow whatever provider is configured in Settings (or the chips' ad-hoc
 * override), which can be a cloud API. Before this, the only place that was
 * visible was inside `LiveActionChipModelPicker`'s popover, which needs an
 * extra click - this surfaces it inline instead.
 *
 * `isLocal` reuses `ALWAYS_AVAILABLE_PROVIDERS` (the same list
 * `LiveActionChipModelPicker` uses to decide which providers work without a
 * saved API key) so "local" here can never drift out of sync with that
 * picker's own notion of local vs. cloud.
 */
export function LiveProviderIndicator({ provider, className }: LiveProviderIndicatorProps) {
  const isLocal = (ALWAYS_AVAILABLE_PROVIDERS as string[]).includes(provider);

  // Local is the default, privacy-first behavior - flagging it on every
  // generation is noise. Only cloud usage (a meaningful exception) surfaces here.
  if (isLocal) return null;

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={cn(
              'inline-flex items-center gap-1 rounded-full border border-border/10 bg-secondary/10 px-2 py-0.5 text-[11px] font-medium whitespace-nowrap text-muted-foreground',
              className
            )}
          >
            <Cloud className="w-3 h-3" />
            {`Cloud · ${providerLabel(provider)}`}
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          {`Transcript is sent to ${providerLabel(provider)}'s API for this generation.`}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
