import { Loader2, Sparkles, AlertCircle } from 'lucide-react';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useConfig } from '@/contexts/ConfigContext';
import { MarkdownContent } from '@/components/MarkdownContent';
import { LiveProviderIndicator } from './LiveProviderIndicator';
import type { UseLiveInsightsResult } from '@/hooks/useLiveInsights';

export function LiveInsightsPanel({ insights, isLoading, error }: UseLiveInsightsResult) {
  const { isRecording } = useRecordingState();
  // No ad-hoc override for this panel (unlike the action chips) - it always
  // runs on the Settings-configured provider, so that's the effective one.
  const { modelConfig } = useConfig();

  const isFirstGeneration = isLoading && !insights;

  return (
    <div className="w-full border-l border-border/10 flex flex-col overflow-y-auto">
      {/* Shares TranscriptPanel's .glass-panel-header so the two headers stay in sync */}
      <div className="glass-panel-header">
        <div className="flex items-center justify-center gap-2">
          <Sparkles className="w-4 h-4 text-accent-violet" />
          <h2 className="text-sm font-semibold text-foreground">Live Insights</h2>
          <LiveProviderIndicator provider={modelConfig.provider} />
        </div>
      </div>

      <div className="pb-20 px-4 pt-4">
        <div className="flex justify-center">
          <div className="w-full max-w-3xl mx-auto">
            {error && (
              <div className="flex items-start gap-2 text-xs text-destructive bg-destructive/10 border border-destructive/20 rounded-md px-3 py-2 mb-3">
                <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
                {/* line-clamp bounds unexpectedly long/malformed backend error text so it can't blow out the panel layout */}
                <span className="line-clamp-3 break-words" title={error}>{error}</span>
              </div>
            )}

            {isFirstGeneration && (
              <div className="flex flex-col items-center justify-center text-center py-12 text-muted-foreground">
                <Loader2 className="w-5 h-5 animate-spin mb-3 text-accent-violet" />
                <p className="text-sm">Generating live insights...</p>
              </div>
            )}

            {!isFirstGeneration && !insights && (
              <div className="flex flex-col items-center justify-center text-center py-12 text-muted-foreground">
                <Sparkles className="w-8 h-8 mb-3 text-accent-violet/40" />
                <p className="text-sm max-w-xs">
                  {isRecording
                    ? 'Live insights will appear here once the meeting has enough content.'
                    : 'Start a recording to see a running summary and action items here.'}
                </p>
              </div>
            )}

            {insights && (
              <div className="glass-card border-accent-violet/20 bg-gradient-to-br from-accent-violet/15 to-secondary/5 p-4">
                <MarkdownContent>{insights}</MarkdownContent>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
