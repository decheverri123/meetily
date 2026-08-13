"use client";
import { useCallback, useState, useEffect, useRef } from 'react';
import { motion } from 'framer-motion';
import { Summary, SummaryDataResponse, SummaryResponse } from '@/types';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { AiChatOverlay } from '@/components/shared/AiChatOverlay';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { useAskPanelShortcut } from '@/hooks/useAskPanelShortcut';
import { useSuggestedQuestions } from '@/hooks/useSuggestedQuestions';
import { useTranscriptSegments } from '@/hooks/useTranscriptSegments';
import { modelConfigLabel } from '@/app/_components/LiveActionChipModelPicker';

// Custom hooks
import { useMeetingData } from '@/hooks/meeting-details/useMeetingData';
import { useSummaryGeneration } from '@/hooks/meeting-details/useSummaryGeneration';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { useCopyOperations } from '@/hooks/meeting-details/useCopyOperations';
import { useMeetingOperations } from '@/hooks/meeting-details/useMeetingOperations';
import { useConfig } from '@/contexts/ConfigContext';

import { useRouter } from 'next/navigation';
import { ConfirmationModal } from '@/components/ConfirmationModel/confirmation-modal';

export default function PageContent({
  meeting,
  summaryData,
  shouldAutoGenerate = false,
  onAutoGenerateComplete,
  onMeetingUpdated,
  onRefetchTranscripts,
  // Pagination props for efficient transcript loading
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
}: {
  meeting: any;
  summaryData: Summary | null;
  shouldAutoGenerate?: boolean;
  onAutoGenerateComplete?: () => void;
  onMeetingUpdated?: () => Promise<void>;
  onRefetchTranscripts?: () => Promise<void>;
  // Pagination props
  segments?: any[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;
}) {
  console.log('📄 PAGE CONTENT: Initializing with data:', {
    meetingId: meeting.id,
    summaryDataKeys: summaryData ? Object.keys(summaryData) : null,
    transcriptsCount: meeting.transcripts?.length
  });

  // State
  const [customPrompt, setCustomPrompt] = useState<string>('');
  const [isRecording] = useState(false);
  const [summaryResponse] = useState<SummaryResponse | null>(null);
  // Ask sidebar: docked beside the note, dismissible, toggled with Cmd/Ctrl+J.
  const [showAskPanel, setShowAskPanel] = useState(true);
  // Collapsed by default: the ask sidebar is the primary surface on this
  // screen now, and the transcript is a reference panel a user opts into.
  const [transcriptCollapsed, setTranscriptCollapsed] = useState(true);
  const [citedSegmentIds, setCitedSegmentIds] = useState<string[]>([]);
  const [focusSegment, setFocusSegment] = useState<{ id: string } | null>(null);
  const [isDeleteModalOpen, setIsDeleteModalOpen] = useState(false);

  const router = useRouter();

  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);

  // Sidebar context
  const { serverAddress, refetchMeetings, setCurrentMeeting } = useSidebar();

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig } = useConfig();

  // Custom hooks
  const meetingData = useMeetingData({ meeting, summaryData, onMeetingUpdated });
  const templates = useTemplates(meeting.id);

  const handleRegisterModalOpen = (openFn: () => void) => {
    console.log('📝 Registering modal open function in PageContent');
    openModelSettingsRef.current = openFn;
  };

  const handleOpenModelSettings = () => {
    console.log('🔔 Opening model settings from PageContent');
    if (openModelSettingsRef.current) {
      openModelSettingsRef.current();
    } else {
      console.warn('⚠️ Modal open function not yet registered');
    }
  };

  const handleSaveModelConfig = async (config?: ModelConfig) => {
    if (!config) return;
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey ?? null,
        ollamaEndpoint: config.ollamaEndpoint ?? null,
      });

      // Emit event so ConfigContext and other listeners stay in sync
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      toast.success('Model settings saved successfully');
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error('Failed to save model settings');
    }
  };

  const summaryGeneration = useSummaryGeneration({
    meeting,
    transcripts: meetingData.transcripts,
    modelConfig: modelConfig,
    isModelConfigLoading: false, // ConfigContext loads on mount
    selectedTemplate: templates.selectedTemplate,
    generatedTemplate: templates.generatedTemplate,
    onTemplateResolved: templates.applyResolvedTemplate,
    onMeetingUpdated,
    updateMeetingTitle: meetingData.updateMeetingTitle,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });

  const copyOperations = useCopyOperations({
    meeting,
    transcripts: meetingData.transcripts,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
  });

  const meetingOperations = useMeetingOperations({
    meeting,
  });

  const handleConfirmDeleteMeeting = async () => {
    setIsDeleteModalOpen(false);
    const success = await meetingOperations.handleDeleteMeeting();
    if (success) {
      refetchMeetings();
      setCurrentMeeting({ id: 'intro-call', title: '+ New Call' });
      router.push('/');
    }
  };

  useAskPanelShortcut(useCallback(() => setShowAskPanel(open => !open), []));

  const meetingSegments = useTranscriptSegments();
  const meetingSuggestions = useSuggestedQuestions({
    command: 'suggest_meeting_questions',
    args: { meetingId: meeting.id },
    scope: meeting.id,
  });
  const meetingAskBuildArgs = useCallback(
    (question: string) => ({ meetingId: meeting.id, question }),
    [meeting.id]
  );
  const askScoped = {
    command: 'ask_about_meeting',
    buildArgs: meetingAskBuildArgs,
    segments: meetingSegments,
    suggestions: meetingSuggestions,
    placeholder: 'Ask a question about this meeting...',
    modelLabel: modelConfigLabel(modelConfig),
    onCitedSegmentsChange: setCitedSegmentIds,
    onFocusSegment: (id: string) => setFocusSegment({ id }),
  };

  useEffect(() => {
    Analytics.trackPageView('meeting_details');
  }, []);

  // Reflect a previously-generated summary's resolved template on load - the
  // generate/regenerate paths already do this via useSummaryGeneration's
  // onTemplateResolved calls, but a plain page load/reload only gets
  // summaryData through this prop, so it needs its own call site. `summaryData`
  // itself may be a fresh object each render (see page.tsx), so the effect is
  // keyed off the resolved-template fields rather than object identity.
  useEffect(() => {
    templates.applyResolvedTemplate(summaryData as SummaryDataResponse | null);
  }, [
    summaryData?.resolved_template_id,
    summaryData?.resolved_template_name,
    summaryData?.is_generated_template,
    meeting.id,
    templates.applyResolvedTemplate,
  ]);

  useEffect(() => {
    let cancelled = false;

    const autoGenerate = async () => {
      if (shouldAutoGenerate && meetingData.transcripts.length > 0 && !cancelled) {
        console.log(`🤖 Auto-generating summary with ${modelConfig.provider}/${modelConfig.model}...`);
        await summaryGeneration.handleGenerateSummary('');

        // Notify parent that auto-generation is complete (only if not cancelled)
        if (onAutoGenerateComplete && !cancelled) {
          onAutoGenerateComplete();
        }
      }
    };

    autoGenerate();

    // Cleanup: cancel if component unmounts or meeting changes
    return () => {
      cancelled = true;
    };
  }, [shouldAutoGenerate, meeting.id]); // Re-run if meeting changes

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="relative flex flex-col h-screen bg-background text-foreground overflow-hidden"
    >
      <div className="pointer-events-none absolute inset-0 overflow-hidden">
        <div className="animate-drift absolute -top-1/4 left-1/3 h-[60vh] w-[60vh] rounded-full bg-primary/10 blur-[120px]" />
        <div className="animate-drift absolute bottom-0 right-0 h-[50vh] w-[50vh] rounded-full bg-accent-violet/10 blur-[120px]" style={{ animationDelay: '6s' }} />
      </div>

      <div className="relative z-10 flex flex-1 overflow-hidden gap-[18px] p-4">
        <TranscriptPanel
          transcripts={meetingData.transcripts}
          customPrompt={customPrompt}
          onPromptChange={setCustomPrompt}
          onCopyTranscript={copyOperations.handleCopyTranscript}
          onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
          isRecording={isRecording}
          disableAutoScroll={true}
          isCollapsed={transcriptCollapsed}
          onToggleCollapse={() => setTranscriptCollapsed(collapsed => !collapsed)}
          // Pagination props for efficient loading
          usePagination={true}
          segments={segments}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          citedSegmentIds={citedSegmentIds}
          focusSegment={focusSegment}
          // Retranscription props
          meetingId={meeting.id}
          meetingFolderPath={meeting.folder_path}
          onRefetchTranscripts={onRefetchTranscripts}
        />
        <SummaryPanel
          meeting={meeting}
          meetingTitle={meetingData.meetingTitle}
          onTitleChange={meetingData.handleTitleChange}
          isEditingTitle={meetingData.isEditingTitle}
          onStartEditTitle={() => meetingData.setIsEditingTitle(true)}
          onFinishEditTitle={() => meetingData.setIsEditingTitle(false)}
          isTitleDirty={meetingData.isTitleDirty}
          summaryRef={meetingData.blockNoteSummaryRef}
          isSaving={meetingData.isSaving}
          onSaveAll={meetingData.saveAllChanges}
          onCopySummary={copyOperations.handleCopySummary}
          onOpenFolder={meetingOperations.handleOpenMeetingFolder}
          aiSummary={meetingData.aiSummary}
          summaryStatus={summaryGeneration.summaryStatus}
          transcripts={meetingData.transcripts}
          modelConfig={modelConfig}
          setModelConfig={setModelConfig}
          onSaveModelConfig={handleSaveModelConfig}
          onGenerateSummary={summaryGeneration.handleGenerateSummary}
          onStopGeneration={summaryGeneration.handleStopGeneration}
          customPrompt={customPrompt}
          summaryResponse={summaryResponse}
          onSaveSummary={meetingData.handleSaveSummary}
          onSummaryChange={meetingData.handleSummaryChange}
          onDirtyChange={meetingData.setIsSummaryDirty}
          summaryError={summaryGeneration.summaryError}
          onRegenerateSummary={summaryGeneration.handleRegenerateSummary}
          getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
          availableTemplates={templates.availableTemplates}
          selectedTemplate={templates.selectedTemplate}
          onTemplateSelect={templates.handleTemplateSelection}
          isModelConfigLoading={false}
          onOpenModelSettings={handleRegisterModalOpen}
          onDeleteMeeting={async () => setIsDeleteModalOpen(true)}
        />
        <AiChatOverlay open={showAskPanel} onOpenChange={setShowAskPanel} scoped={askScoped} />
      </div>

      <ConfirmationModal
        isOpen={isDeleteModalOpen}
        text="Are you sure you want to delete this meeting? All associated transcripts and summaries will be permanently deleted."
        onConfirm={handleConfirmDeleteMeeting}
        onCancel={() => setIsDeleteModalOpen(false)}
      />
    </motion.div>
  );
}
