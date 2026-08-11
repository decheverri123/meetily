import { Cloud, ShieldCheck } from 'lucide-react';
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
 * Always-visible "Local" vs "Cloud (Provider)" badge for the live action
 * chips and Live Insights panel.
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

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={cn(
              'inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium whitespace-nowrap',
              isLocal
                ? 'border-success/20 bg-success/10 text-success'
                : 'border-border/10 bg-secondary/10 text-muted-foreground',
              className
            )}
          >
            {isLocal ? <ShieldCheck className="w-3 h-3" /> : <Cloud className="w-3 h-3" />}
            {isLocal ? 'Local' : `Cloud · ${providerLabel(provider)}`}
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          {isLocal
            ? 'Transcript stays on this device (Built-in AI / Ollama).'
            : `Transcript is sent to ${providerLabel(provider)}'s API for this generation.`}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
