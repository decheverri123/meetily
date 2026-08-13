import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { PermissionWarning } from '@/components/PermissionWarning';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Copy, GlobeIcon, PanelLeftClose } from 'lucide-react';
import { CollapsedPanelRail } from '@/components/shared/CollapsedPanelRail';
import { CitedSourcesPill } from '@/components/shared/CitedSourcesPill';
import { cn } from '@/lib/utils';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { ModalType } from '@/hooks/useModalState';
import { useIsLinux } from '@/hooks/usePlatform';
import { useTranscriptSegments } from '@/hooks/useTranscriptSegments';

/**
 * TranscriptPanel Component
 *
 * Displays transcript content with controls for copying and language settings.
 * Uses TranscriptContext, ConfigContext, and RecordingStateContext internally.
 */

interface TranscriptPanelProps {
  // indicates stop-processing state for transcripts; derived from backend statuses.
  isProcessingStop: boolean;
  isStopping: boolean;
  showModal: (name: ModalType, message?: string) => void;
  /** Collapses the panel to a narrow rail; the caller owns the state. */
  isCollapsed?: boolean;
  onToggleCollapse?: () => void;
  /** Segments the live ask panel's latest answer cited. */
  citedSegmentIds?: readonly string[];
  /** Segment a citation chip was clicked on, to scroll into view. */
  focusSegment?: { id: string } | null;
}

export function TranscriptPanel({
  isProcessingStop,
  isStopping,
  showModal,
  isCollapsed = false,
  onToggleCollapse,
  citedSegmentIds,
  focusSegment = null,
}: TranscriptPanelProps) {
  // Contexts
  const { transcripts, transcriptContainerRef, copyTranscript } = useTranscripts();
  const { transcriptModelConfig } = useConfig();
  const { isRecording, isPaused } = useRecordingState();
  const { checkPermissions, isChecking, hasSystemAudio, hasMicrophone } = usePermissionCheck();
  const isLinux = useIsLinux();

  const segments = useTranscriptSegments();

  const collapsed = Boolean(onToggleCollapse) && isCollapsed;

  return (
    <div className="relative flex h-full w-full overflow-hidden">
      {/* min-w keeps the transcript from reflowing to rail width mid-collapse;
          the container clips it instead, so it slides out rather than squashing. */}
      <div
        ref={transcriptContainerRef}
        className={cn(
          'flex w-full min-w-[420px] flex-col overflow-y-auto border-r border-border/10 transition-opacity duration-200 motion-reduce:transition-none',
          // Waits for the column to widen before fading back in, so the
          // transcript is never visible clipped to rail width.
          collapsed ? 'pointer-events-none opacity-0' : 'delay-100'
        )}
      >
      <div className="glass-panel-header">
        <div className="flex flex-col space-y-3">
          <div className="flex  flex-col space-y-2">
            <div className="flex justify-center  items-center space-x-2">
              <ButtonGroup>
                {onToggleCollapse && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={onToggleCollapse}
                    title="Hide Transcript"
                    aria-label="Hide transcript"
                  >
                    <PanelLeftClose />
                  </Button>
                )}
                {transcripts?.length > 0 && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={copyTranscript}
                    title="Copy Transcript"
                  >
                    <Copy />
                    <span className='hidden md:inline'>
                      Copy
                    </span>
                  </Button>
                )}
                {transcriptModelConfig.provider === "localWhisper" &&
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => showModal('languageSettings')}
                    title="Language"
                  >
                    <GlobeIcon />
                    <span className='hidden md:inline'>
                      Language
                    </span>
                  </Button>
                }
              </ButtonGroup>
            </div>
            <div className="flex justify-center empty:hidden">
              <CitedSourcesPill count={citedSegmentIds?.length ?? 0} />
            </div>
          </div>
        </div>
      </div>

      {/* Permission Warning - Not needed on Linux */}
      {!isRecording && !isChecking && !isLinux && (
        <div className="flex justify-center px-4 pt-4">
          <PermissionWarning
            hasMicrophone={hasMicrophone}
            hasSystemAudio={hasSystemAudio}
            onRecheck={checkPermissions}
            isRechecking={isChecking}
          />
        </div>
      )}

      <div className="pb-20">
        <div className="flex justify-center">
          <div className="max-w-3xl mx-auto">
            <VirtualizedTranscriptView
              segments={segments}
              isRecording={isRecording}
              isPaused={isPaused}
              isProcessing={isProcessingStop}
              isStopping={isStopping}
              enableStreaming={isRecording}
              showConfidence={true}
              highlightedSegmentIds={citedSegmentIds}
              focusSegment={focusSegment}
            />
          </div>
        </div>
      </div>
      </div>

      {onToggleCollapse && (
        <CollapsedPanelRail
          label="Transcript"
          meta={`${segments.length}`}
          visible={collapsed}
          onExpand={onToggleCollapse}
          expandTitle="Show transcript"
        />
      )}
    </div>
  );
}
