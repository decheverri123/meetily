'use client';

import { useCallback, useState, useEffect, useRef, type PointerEvent as ReactPointerEvent } from 'react';
import { motion } from 'framer-motion';
import { Sparkles } from 'lucide-react';
import { RecordingControls } from '@/components/RecordingControls';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { StatusOverlays } from '@/app/_components/StatusOverlays';
import { IdleHome } from '@/app/_components/IdleHome';
import Analytics from '@/lib/analytics';
import { SettingsModals } from './_components/SettingsModal';
import { TranscriptPanel } from './_components/TranscriptPanel';
import { LiveInsightsPanel } from './_components/LiveInsightsPanel';
import { LiveAskPanel } from './_components/LiveAskPanel';
import { AskFloatingBubble } from './_components/AskFloatingBubble';
import { useModalState } from '@/hooks/useModalState';
import { useRecordingStateSync } from '@/hooks/useRecordingStateSync';
import { useRecordingStart } from '@/hooks/useRecordingStart';
import { useRecordingStop } from '@/hooks/useRecordingStop';
import { useTranscriptRecovery } from '@/hooks/useTranscriptRecovery';
import { useLiveInsights } from '@/hooks/useLiveInsights';
import { useLiveActionChips, LiveActionChipModelOverride } from '@/hooks/useLiveActionChips';
import { useAskPanelShortcut } from '@/hooks/useAskPanelShortcut';
import { TranscriptRecovery } from '@/components/TranscriptRecovery';
import { indexedDBService } from '@/services/indexedDBService';
import { toast } from 'sonner';
import { useRouter } from 'next/navigation';

export default function Home() {
  // Local page state (not moved to contexts)
  const [isRecording, setIsRecordingState] = useState(false);
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);
  // Live Insights panel - open by default, the primary reason to be on this screen.
  const [showLiveInsights, setShowLiveInsights] = useState(true);
  // Ad-hoc, session-only override of which provider/model powers live action
  // chip generation - set via LiveActionChipModelPicker. Deliberately not
  // persisted: null means "use the Settings-configured provider/model", which
  // must stay the default (see useLiveActionChips's modelOverride param).
  const [liveActionChipOverride, setLiveActionChipOverride] = useState<LiveActionChipModelOverride | null>(null);
  // Ask panel: a floating chat bubble, collapsed by default, toggled with
  // Cmd/Ctrl+J or by clicking the bubble.
  const [showAskPanel, setShowAskPanel] = useState(false);
  // Flags the bubble while collapsed - cleared as soon as it's opened.
  const [hasAskUnread, setHasAskUnread] = useState(false);
  // Transcript segments the latest answer cited, and the one whose chip was
  // last clicked. Held here because they are produced by the ask panel and
  // consumed by the transcript column, which are otherwise unrelated.
  // Collapsed by default: Live Insights is the primary surface on this
  // screen now, and the transcript is a reference panel a user opts into.
  const [transcriptCollapsed, setTranscriptCollapsed] = useState(true);
  const [citedSegmentIds, setCitedSegmentIds] = useState<string[]>([]);
  const [focusSegment, setFocusSegment] = useState<{ id: string } | null>(null);
  // Transcript's share of the transcript+insights row when both are open;
  // dragged via the divider between them.
  const [transcriptSplit, setTranscriptSplit] = useState(0.5);
  const [isDraggingSplit, setIsDraggingSplit] = useState(false);
  const splitRowRef = useRef<HTMLDivElement>(null);

  // Use contexts for state management
  const { meetingTitle, transcripts } = useTranscripts();
  const { transcriptModelConfig, selectedDevices } = useConfig();
  const recordingState = useRecordingState();

  // Extract status from global state
  const { status, isStopping, isProcessing, isSaving } = recordingState;

  // Hooks
  const { hasMicrophone, hasSystemAudio, isChecking, checkPermissions } = usePermissionCheck();
  const { setIsMeetingActive, isCollapsed: sidebarCollapsed, refetchMeetings } = useSidebar();
  const { modals, messages, showModal, hideModal } = useModalState(transcriptModelConfig);
  const { isRecordingDisabled, setIsRecordingDisabled } = useRecordingStateSync(isRecording, setIsRecordingState, setIsMeetingActive);
  const { handleRecordingStart } = useRecordingStart(isRecording, setIsRecordingState, showModal);

  // Get handleRecordingStop function and setIsStopping (state comes from global context)
  const { handleRecordingStop, setIsStopping } = useRecordingStop(
    setIsRecordingState,
    setIsRecordingDisabled
  );

  // Recovery hook
  const {
    recoverableMeetings,
    isLoading: isLoadingRecovery,
    isRecovering,
    checkForRecoverableTranscripts,
    recoverMeeting,
    loadMeetingTranscripts,
    deleteRecoverableMeeting
  } = useTranscriptRecovery();

  const router = useRouter();

  // Called unconditionally (not gated on `showLiveInsights`) so its internal
  // state - generated insights, growth tracking, epoch guard - stays mounted
  // and survives the user toggling the Live Insights panel off and back on.
  const liveInsights = useLiveInsights();

  // Called unconditionally for the same reason as useLiveInsights() above -
  // keeps per-chip loading/result/error state mounted for the life of the
  // meeting regardless of any panel visibility.
  const liveActionChips = useLiveActionChips(liveActionChipOverride);

  const toggleAskPanel = useCallback(() => {
    setShowAskPanel(open => {
      const next = !open;
      if (next) setHasAskUnread(false);
      return next;
    });
  }, []);
  useAskPanelShortcut(toggleAskPanel);

  // Shared by both LiveAskPanel triggers (a finished answer, a new suggestion
  // batch) - either one flags the bubble while it's collapsed.
  const flagAskUnread = useCallback(() => {
    setHasAskUnread(current => current || !showAskPanel);
  }, [showAskPanel]);

  // Drag-to-resize the transcript/insights split. Listens on `window` rather
  // than the divider itself so the drag keeps tracking even if the pointer
  // outruns the thin handle mid-move.
  const handleSplitPointerDown = useCallback((event: ReactPointerEvent) => {
    event.preventDefault();
    const row = splitRowRef.current;
    if (!row) return;
    setIsDraggingSplit(true);

    const handleMove = (moveEvent: PointerEvent) => {
      const rect = row.getBoundingClientRect();
      const ratio = (moveEvent.clientX - rect.left) / rect.width;
      setTranscriptSplit(Math.min(0.75, Math.max(0.25, ratio)));
    };
    const handleUp = () => {
      setIsDraggingSplit(false);
      window.removeEventListener('pointermove', handleMove);
      window.removeEventListener('pointerup', handleUp);
    };
    window.addEventListener('pointermove', handleMove);
    window.addEventListener('pointerup', handleUp);
  }, []);

  useEffect(() => {
    // Track page view
    Analytics.trackPageView('home');
  }, []);

  // Startup recovery check
  useEffect(() => {
    const performStartupChecks = async () => {
      try {
        // Skip recovery check if currently recording or processing stop
        // This prevents the recovery dialog from showing when:
        if (recordingState.isRecording ||
          status === RecordingStatus.STOPPING ||
          status === RecordingStatus.PROCESSING_TRANSCRIPTS ||
          status === RecordingStatus.SAVING) {
          console.log('Skipping recovery check - recording in progress or processing');
          return;
        }

        // 1. Clean up old meetings (7+ days)
        try {
          await indexedDBService.deleteOldMeetings(7);
        } catch (error) {
          console.warn('⚠️ Failed to clean up old meetings:', error);
        }

        // 2. Clean up saved meetings (24+ hours after save)
        try {
          await indexedDBService.deleteSavedMeetings(24);
        } catch (error) {
          console.warn('⚠️ Failed to clean up saved meetings:', error);
        }

        // 3. Always check for recoverable meetings on startup
        // Don't skip based on sessionStorage - we need to check every time
        await checkForRecoverableTranscripts();
      } catch (error) {
        console.error('Failed to perform startup checks:', error);
      }
    };

    performStartupChecks();
  }, [checkForRecoverableTranscripts, recordingState.isRecording, status]);

  // Watch for recoverable meetings changes and show dialog once per session
  useEffect(() => {
    // Only show dialog if we have meetings and haven't shown it yet this session
    if (recoverableMeetings.length > 0) {
      const shownThisSession = sessionStorage.getItem('recovery_dialog_shown');
      if (!shownThisSession) {
        setShowRecoveryDialog(true);
        sessionStorage.setItem('recovery_dialog_shown', 'true');
      }
    }
  }, [recoverableMeetings]);

  // Handle recovery with toast notifications and navigation
  const handleRecovery = async (meetingId: string) => {
    try {
      const result = await recoverMeeting(meetingId);

      if (result.success) {
        toast.success('Meeting recovered successfully!', {
          description: result.audioRecoveryStatus?.status === 'success'
            ? 'Transcripts and audio recovered'
            : 'Transcripts recovered (no audio available)',
          action: result.meetingId ? {
            label: 'View Meeting',
            onClick: () => {
              router.push(`/meeting-details?id=${result.meetingId}`);
            }
          } : undefined,
          duration: 10000,
        });

        // Refresh sidebar to show the newly recovered meeting
        await refetchMeetings();

        // If no more recoverable meetings, clear session flag so dialog can show again
        if (recoverableMeetings.length === 0) {
          sessionStorage.removeItem('recovery_dialog_shown');
        }

        // Auto-navigate after a short delay
        if (result.meetingId) {
          setTimeout(() => {
            router.push(`/meeting-details?id=${result.meetingId}`);
          }, 2000);
        }
      }
    } catch (error) {
      toast.error('Failed to recover meeting', {
        description: error instanceof Error ? error.message : 'Unknown error occurred',
      });
      throw error;
    }
  };

  // Handle dialog close - clear session flag if no meetings left
  const handleDialogClose = () => {
    setShowRecoveryDialog(false);
    // If user closes dialog and there are no more meetings, clear the flag
    // This allows the dialog to show again next session if new meetings appear
    if (recoverableMeetings.length === 0) {
      sessionStorage.removeItem('recovery_dialog_shown');
    }
  };

  // Computed values using global status
  const isProcessingStop = status === RecordingStatus.PROCESSING_TRANSCRIPTS || isProcessing;

  // Idle = no recording session in any phase and nothing transcribed yet. Only
  // this state swaps in IdleHome; every recording phase keeps the live UI below.
  const isIdle =
    !isRecording &&
    !recordingState.isRecording &&
    !isStopping &&
    !isProcessingStop &&
    !isSaving &&
    status !== RecordingStatus.STARTING &&
    transcripts.length === 0;

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="relative flex flex-col h-screen bg-background text-foreground overflow-hidden"
    >
      {/* Ambient background glow - purely decorative, sits behind all content */}
      <div className="pointer-events-none absolute inset-0 overflow-hidden">
        <div className="animate-drift absolute -top-1/4 left-1/3 h-[60vh] w-[60vh] rounded-full bg-primary/10 blur-[120px]" />
        <div className="animate-drift absolute bottom-0 right-0 h-[50vh] w-[50vh] rounded-full bg-accent-violet/10 blur-[120px]" style={{ animationDelay: '6s' }} />
      </div>

      {/* All Modals supported*/}
      <SettingsModals
        modals={modals}
        messages={messages}
        onClose={hideModal}
      />

      {/* Recovery Dialog */}
      <TranscriptRecovery
        isOpen={showRecoveryDialog}
        onClose={handleDialogClose}
        recoverableMeetings={recoverableMeetings}
        onRecover={handleRecovery}
        onDelete={deleteRecoverableMeeting}
        onLoadPreview={loadMeetingTranscripts}
      />
      <div className="flex flex-1 overflow-hidden">
        {isIdle ? (
          <IdleHome
            onStartRecording={handleRecordingStart}
            showModal={showModal}
            hasMicrophone={hasMicrophone}
            hasSystemAudio={hasSystemAudio}
            isCheckingPermissions={isChecking}
            onRecheckPermissions={checkPermissions}
          />
        ) : (
          <>
            <div ref={splitRowRef} className="flex min-w-0 flex-1 overflow-hidden">
              {/* Animates flex-basis/grow rather than width: this column is
                  flex-sized, so `width` never resolves to a value that could be
                  interpolated. Both endpoints are lengths, so they tween. */}
              <div
                className={cn(
                  'flex flex-col overflow-hidden ease-out motion-reduce:transition-none',
                  !isDraggingSplit && 'transition-[flex-basis,flex-grow] duration-300',
                  transcriptCollapsed ? 'grow-0 basis-[44px] p-4' : 'grow'
                )}
                style={
                  !transcriptCollapsed && showLiveInsights
                    ? { flexBasis: `${transcriptSplit * 100}%` }
                    : undefined
                }
              >
                <TranscriptPanel
                  isProcessingStop={isProcessingStop}
                  isStopping={isStopping}
                  showModal={showModal}
                  isCollapsed={transcriptCollapsed}
                  onToggleCollapse={() => setTranscriptCollapsed(collapsed => !collapsed)}
                  citedSegmentIds={citedSegmentIds}
                  focusSegment={focusSegment}
                />
              </div>

              {/* Draggable divider - only meaningful (and shown) once both
                  neighbors are actually visible and share the row. */}
              {!transcriptCollapsed && showLiveInsights && (
                <div
                  onPointerDown={handleSplitPointerDown}
                  className="group relative z-10 -mx-1.5 w-3 shrink-0 cursor-col-resize touch-none"
                >
                  <div
                    className={cn(
                      'absolute inset-y-6 left-1/2 w-px -translate-x-1/2 rounded-full bg-border/10 transition-colors group-hover:bg-primary/50',
                      isDraggingSplit && 'bg-primary/60'
                    )}
                  />
                </div>
              )}

              {/* Same slide-shut technique as the transcript column: the outer
                  clip tweens flex-basis while the inner content holds its
                  expanded min-width and crossfades, so it slides out rather
                  than squashing its contents mid-animation. */}
              <div
                className={cn(
                  'flex flex-col overflow-hidden ease-out motion-reduce:transition-none',
                  !isDraggingSplit && 'transition-[flex-basis,flex-grow] duration-300',
                  showLiveInsights ? 'grow' : 'grow-0 basis-0'
                )}
                style={
                  showLiveInsights && !transcriptCollapsed
                    ? { flexBasis: `${(1 - transcriptSplit) * 100}%` }
                    : undefined
                }
              >
                <div
                  className={cn(
                    'flex h-full min-w-[420px] flex-col transition-opacity duration-200 motion-reduce:transition-none',
                    showLiveInsights ? 'opacity-100 delay-100' : 'pointer-events-none opacity-0'
                  )}
                >
                  <LiveInsightsPanel {...liveInsights} />
                </div>
              </div>
            </div>

            <AskFloatingBubble open={showAskPanel} hasUnread={hasAskUnread} onOpen={toggleAskPanel}>
              <LiveAskPanel
                onCitedSegmentsChange={setCitedSegmentIds}
                onFocusSegment={id => setFocusSegment({ id })}
                onClose={() => setShowAskPanel(false)}
                onAnswered={flagAskUnread}
                onSuggestionsReady={flagAskUnread}
                liveActionChips={liveActionChips}
                liveActionChipOverride={liveActionChipOverride}
                onLiveActionChipOverrideChange={setLiveActionChipOverride}
                isRecording={recordingState.isRecording}
              />
            </AskFloatingBubble>
          </>
        )}

        {/* Recording controls - only show when permissions are granted or already recording and not showing status messages.
            Hidden while idle: IdleHome's "Record now" button is the start affordance there. */}
        {!isIdle &&
          (hasMicrophone || isRecording) &&
          status !== RecordingStatus.PROCESSING_TRANSCRIPTS &&
          status !== RecordingStatus.SAVING && (
            <div className="fixed bottom-12 left-0 right-0 z-10">
              <div
                className="flex justify-center pl-8 transition-[margin] duration-300"
                style={{
                  marginLeft: sidebarCollapsed ? '4rem' : '16rem'
                }}
              >
                <div className="w-2/3 max-w-[750px] flex justify-center items-center gap-2">
                  <RecordingControls
                    isRecording={recordingState.isRecording}
                    onRecordingStop={(callApi = true) => handleRecordingStop(callApi)}
                    onRecordingStart={handleRecordingStart}
                    onTranscriptReceived={() => { }} // Not actually used by RecordingControls
                    onStopInitiated={() => setIsStopping(true)}
                    onTranscriptionError={(message) => {
                      showModal('errorAlert', message);
                    }}
                    isRecordingDisabled={isRecordingDisabled}
                    isParentProcessing={isProcessingStop}
                    selectedDevices={selectedDevices}
                    meetingName={meetingTitle}
                  />
                  <Button
                    type="button"
                    variant={showLiveInsights ? 'default' : 'outline'}
                    size="icon"
                    className={cn('rounded-full h-9 w-9', !showLiveInsights && 'glass-pill')}
                    onClick={() => setShowLiveInsights(prev => !prev)}
                    title={showLiveInsights ? 'Hide Live Insights' : 'Show Live Insights'}
                  >
                    <Sparkles className="w-4 h-4" />
                  </Button>
                </div>
              </div>
            </div>
          )}

        {/* Status Overlays - Processing and Saving */}
        <StatusOverlays
          isProcessing={status === RecordingStatus.PROCESSING_TRANSCRIPTS && !recordingState.isRecording}
          isSaving={status === RecordingStatus.SAVING}
          sidebarCollapsed={sidebarCollapsed}
        />
      </div>
    </motion.div>
  );
}
