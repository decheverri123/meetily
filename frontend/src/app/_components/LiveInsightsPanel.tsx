import { Loader2, Sparkles, AlertCircle } from 'lucide-react';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { MarkdownContent } from '@/components/MarkdownContent';
import type { UseLiveInsightsResult } from '@/hooks/useLiveInsights';

/**
 * LiveInsightsPanel Component
 *
 * Shows a periodically refreshed running summary + action-items list generated
 * from the transcript-so-far while a meeting is actively being recorded.
 * Optional/opt-in second column alongside TranscriptPanel.
 *
 * Purely presentational: `useLiveInsights()` is called by the parent and kept
 * mounted for the life of the meeting, so its state (insights/growth
 * tracking/epoch) survives this panel being toggled off and back on.
 */
export function LiveInsightsPanel({ insights, isLoading, error }: UseLiveInsightsResult) {
  const { isRecording } = useRecordingState();

  const isFirstGeneration = isLoading && !insights;

  return (
    <div className="w-full border-l border-gray-200 bg-white flex flex-col overflow-y-auto">
      {/* Title area - Sticky header, mirrors TranscriptPanel's header styling */}
      <div className="sticky top-0 z-10 bg-white p-4 border-b border-gray-200">
        <div className="flex items-center justify-center space-x-2">
          <Sparkles className="w-4 h-4 text-gray-500" />
          <h2 className="text-sm font-semibold text-gray-700">Live Insights</h2>
        </div>
      </div>

      <div className="pb-20 px-4 pt-4">
        <div className="flex justify-center">
          <div className="w-full max-w-[750px]">
            {error && (
              <div className="flex items-start gap-2 text-xs text-amber-600 bg-amber-50 border border-amber-200 rounded-md px-3 py-2 mb-3">
                <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
                {/* line-clamp bounds unexpectedly long/malformed backend error text so it can't blow out the panel layout */}
                <span className="line-clamp-3 break-words" title={error}>{error}</span>
              </div>
            )}

            {isFirstGeneration && (
              <div className="flex flex-col items-center justify-center text-center py-12 text-gray-400">
                <Loader2 className="w-5 h-5 animate-spin mb-3" />
                <p className="text-sm">Generating live insights...</p>
              </div>
            )}

            {!isFirstGeneration && !insights && (
              <div className="flex flex-col items-center justify-center text-center py-12 text-gray-400">
                <Sparkles className="w-8 h-8 mb-3 text-gray-300" />
                <p className="text-sm max-w-xs">
                  {isRecording
                    ? 'Live insights will appear here once the meeting has enough content.'
                    : 'Start a recording to see a running summary and action items here.'}
                </p>
              </div>
            )}

            {insights && <MarkdownContent>{insights}</MarkdownContent>}
          </div>
        </div>
      </div>
    </div>
  );
}
