"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { TranscriptView } from '@/components/TranscriptView';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { PanelLeftClose } from 'lucide-react';
import { CollapsedPanelRail } from '@/components/shared/CollapsedPanelRail';
import { CitedSourcesPill } from '@/components/shared/CitedSourcesPill';
import { cn } from '@/lib/utils';
import { useMemo } from 'react';

interface TranscriptPanelProps {
  transcripts: Transcript[];
  customPrompt: string;
  onPromptChange: (value: string) => void;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  /** Collapses the panel to a narrow rail; the caller owns the state. */
  isCollapsed?: boolean;
  onToggleCollapse?: () => void;

  /** Segments the ask sidebar's latest answer cited. */
  citedSegmentIds?: readonly string[];
  /** Segment a citation chip was clicked on, to scroll into view. */
  focusSegment?: { id: string } | null;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
}

export function TranscriptPanel({
  transcripts,
  customPrompt,
  onPromptChange,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  isCollapsed = false,
  onToggleCollapse,
  citedSegmentIds,
  focusSegment = null,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
}: TranscriptPanelProps) {
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
    }));
  }, [transcripts, usePagination, segments]);

  const segmentCount = usePagination
    ? (totalCount ?? convertedSegments.length)
    : (transcripts?.length || 0);

  const collapsible = Boolean(onToggleCollapse);
  const collapsed = collapsible && isCollapsed;

  return (
    <div
      className={cn(
        'hidden md:flex relative shrink-0 flex-col overflow-hidden glass-panel transition-[width] duration-300 ease-out motion-reduce:transition-none',
        collapsed ? 'w-11' : 'w-80 lg:w-[380px]'
      )}
    >
      {/* Holds its expanded width while the container clips it, so the panel
          slides out of view instead of reflowing down to rail width. */}
      <div
        className={cn(
          'flex h-full w-80 lg:w-[380px] shrink-0 flex-col transition-opacity duration-200 motion-reduce:transition-none',
          // Waits for the container to widen before fading back in, so the
          // panel is never visible clipped to rail width.
          collapsed ? 'pointer-events-none opacity-0' : 'delay-100'
        )}
      >
      <div className="flex items-center gap-3 p-4 border-b border-border/10">
        {onToggleCollapse && (
          <button
            type="button"
            onClick={onToggleCollapse}
            title="Hide transcript"
            aria-label="Hide transcript"
            className="text-muted-foreground transition-colors hover:text-foreground"
          >
            <PanelLeftClose className="h-4 w-4" />
          </button>
        )}
        <div className="min-w-0">
          <h2 className="text-[13px] font-semibold leading-tight text-foreground">Transcript</h2>
          <p className="mt-0.5 font-mono text-[11px] leading-tight text-muted-foreground">
            {segmentCount} {segmentCount === 1 ? 'segment' : 'segments'}
          </p>
        </div>

        <TranscriptButtonGroup
          transcriptCount={segmentCount}
          onCopyTranscript={onCopyTranscript}
          onOpenMeetingFolder={onOpenMeetingFolder}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onRefetchTranscripts={onRefetchTranscripts}
        />
      </div>

      <div className="px-4 pt-3 empty:hidden">
        <CitedSourcesPill count={citedSegmentIds?.length ?? 0} />
      </div>

      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex-1 overflow-hidden pb-4">
        <VirtualizedTranscriptView
          segments={convertedSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          highlightedSegmentIds={citedSegmentIds}
          focusSegment={focusSegment}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
        />
      </div>

      {/* Custom prompt input at bottom of transcript section */}
      {!isRecording && convertedSegments.length > 0 && (
        <div className="p-2 border-t border-border/10">
          <div className="glass-dashed p-2">
            <textarea
              placeholder="Add context for AI summary. For example people involved, meeting overview, objective etc..."
              className="w-full bg-transparent text-muted-foreground placeholder:text-muted-foreground/60 border-0 focus:outline-none focus:ring-1 focus:ring-primary text-sm min-h-[80px] resize-y"
              value={customPrompt}
              onChange={(e) => onPromptChange(e.target.value)}
            />
          </div>
        </div>
      )}
      </div>

      {collapsible && onToggleCollapse && (
        <CollapsedPanelRail
          label="Transcript"
          meta={`${segmentCount}`}
          visible={collapsed}
          onExpand={onToggleCollapse}
          expandTitle="Show transcript"
        />
      )}
    </div>
  );
}
