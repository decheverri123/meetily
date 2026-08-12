import React, { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import {
  Upload,
  Globe,
  Loader2,
  AlertCircle,
  CheckCircle2,
  X,
  Cpu,
  FileAudio,
  Clock,
  HardDrive,
  ChevronDown,
  ChevronUp,
  Youtube,
  ListPlus,
  Trash2,
  Play,
} from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import { Button } from '../ui/button';
import { Input } from '../ui/input';
import { Textarea } from '../ui/textarea';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '../ui/tabs';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { useImportAudio, ImportResult } from '@/hooks/useImportAudio';
import { useYoutubeImport, YoutubeImportResult } from '@/hooks/useYoutubeImport';
import { useYoutubeBatchImport, BatchItemStatus } from '@/hooks/useYoutubeBatchImport';
import { useRouter } from 'next/navigation';
import { useSidebar } from '../Sidebar/SidebarProvider';
import { LANGUAGES } from '@/constants/languages';
import { useTranscriptionModels, ModelOption } from '@/hooks/useTranscriptionModels';

type ImportTab = 'upload' | 'youtube';
type YoutubeSubTab = 'single' | 'batch';


interface ImportAudioDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  preselectedFile?: string | null;
  onComplete?: () => void;
}

function formatDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);

  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }
  return `${minutes}:${secs.toString().padStart(2, '0')}`;
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function BatchItemStatusIcon({ status }: { status: BatchItemStatus }) {
  switch (status) {
    case 'pending':
      return <span className="h-3.5 w-3.5 rounded-full border border-muted-foreground/40 flex-shrink-0" />;
    case 'downloading':
    case 'transcribing':
      return <Loader2 className="h-3.5 w-3.5 animate-spin text-primary flex-shrink-0" />;
    case 'downloaded':
      return <CheckCircle2 className="h-3.5 w-3.5 text-muted-foreground flex-shrink-0" />;
    case 'complete':
      return <CheckCircle2 className="h-3.5 w-3.5 text-success flex-shrink-0" />;
    case 'failed':
      return <AlertCircle className="h-3.5 w-3.5 text-destructive flex-shrink-0" />;
    case 'cancelled':
      return <X className="h-3.5 w-3.5 text-muted-foreground flex-shrink-0" />;
  }
}

export function ImportAudioDialog({
  open,
  onOpenChange,
  preselectedFile,
  onComplete,
}: ImportAudioDialogProps) {
  const router = useRouter();
  const { refetchMeetings } = useSidebar();
  const { selectedLanguage, transcriptModelConfig } = useConfig();

  const [title, setTitle] = useState('');
  const [selectedLang, setSelectedLang] = useState(selectedLanguage || 'auto');
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [titleModifiedByUser, setTitleModifiedByUser] = useState(false);

  const [activeTab, setActiveTab] = useState<ImportTab>('upload');
  const [youtubeSubTab, setYoutubeSubTab] = useState<YoutubeSubTab>('single');
  const [youtubeUrl, setYoutubeUrl] = useState('');
  const [youtubeTitle, setYoutubeTitle] = useState('');
  const [youtubeTitleModifiedByUser, setYoutubeTitleModifiedByUser] = useState(false);
  const [batchInput, setBatchInput] = useState('');

  // Always start as false — represents "dialog has not yet been opened".
  // Do NOT initialize from the `open` prop: if the component mounts with open=true
  // (e.g. drag-drop path), we still need the initialization effect to run.
  const prevOpenRef = useRef(false);

  // Use centralized model fetching hook
  const {
    availableModels,
    selectedModelKey,
    setSelectedModelKey,
    loadingModels,
    fetchModels,
    resetSelection,
  } = useTranscriptionModels(transcriptModelConfig);

  const handleImportComplete = useCallback((result: ImportResult) => {
    toast.success('Import complete!');

    // Refresh meetings list then navigate to the imported meeting
    refetchMeetings();
    onComplete?.();
    onOpenChange(false);
    router.push(`/meeting-details?id=${result.meeting_id}`);
  }, [router, refetchMeetings, onComplete, onOpenChange]);

  const handleImportError = useCallback((error: string) => {
    toast.error('Import failed', { description: error });
  }, []);

  const {
    status,
    fileInfo,
    progress,
    error,
    isProcessing,
    isBusy,
    selectFile,
    validateFile,
    startImport,
    cancelImport,
    reset,
  } = useImportAudio({
    onComplete: handleImportComplete,
    onError: handleImportError,
  });

  const handleYoutubeImportComplete = useCallback((result: YoutubeImportResult) => {
    toast.success('Import complete!');

    refetchMeetings();
    onComplete?.();
    onOpenChange(false);
    router.push(`/meeting-details?id=${result.meeting_id}`);
  }, [router, refetchMeetings, onComplete, onOpenChange]);

  const handleYoutubeImportError = useCallback((error: string) => {
    toast.error('Import failed', { description: error });
  }, []);

  const {
    status: youtubeStatus,
    videoInfo,
    progress: youtubeProgress,
    error: youtubeError,
    isProcessing: youtubeIsProcessing,
    validateUrl,
    startImport: startYoutubeImport,
    cancelImport: cancelYoutubeImport,
    reset: resetYoutube,
  } = useYoutubeImport({
    onComplete: handleYoutubeImportComplete,
    onError: handleYoutubeImportError,
  });

  const resetYoutubeFlow = useCallback(() => {
    resetYoutube();
    setYoutubeUrl('');
    setYoutubeTitle('');
    setYoutubeTitleModifiedByUser(false);
  }, [resetYoutube]);

  const {
    queue: batchQueue,
    setQueue: setBatchQueue,
    status: batchStatus,
    items: batchItems,
    completed: batchCompleted,
    failed: batchFailed,
    isProcessing: batchIsProcessing,
    isFinished: batchIsFinished,
    error: batchError,
    startBatch,
    cancelBatch,
    reset: resetBatch,
    parseQueue,
  } = useYoutubeBatchImport();

  const batchValidCount = useMemo(
    () => batchQueue.filter((q) => q.valid).length,
    [batchQueue]
  );

  const handleBatchQueueChange = useCallback(
    (text: string) => {
      setBatchInput(text);
      setBatchQueue(parseQueue(text));
    },
    [parseQueue, setBatchQueue]
  );

  const handleStartBatch = useCallback(async () => {
    const titles = batchQueue
      .filter((q) => q.valid)
      .map((q) => (q.title?.trim() ? q.title.trim() : null));
    await startBatch(titles);
  }, [batchQueue, startBatch]);

  const handleBatchComplete = useCallback(
    (completedCount: number, failedCount: number, cancelled: boolean) => {
      if (cancelled) {
        toast.info('Batch cancelled', {
          description: `${completedCount} completed, ${failedCount} failed before cancel`,
        });
        return;
      }
      if (failedCount > 0) {
        toast.warning(`Batch finished with errors`, {
          description: `${completedCount} completed, ${failedCount} failed`,
        });
      } else {
        toast.success(`Batch complete: ${completedCount} imported`);
      }
      refetchMeetings();
    },
    [refetchMeetings]
  );

  useEffect(() => {
    if (batchIsFinished) {
      handleBatchComplete(batchCompleted, batchFailed, batchStatus === 'idle');
    }
  }, [batchIsFinished, batchCompleted, batchFailed, batchStatus, handleBatchComplete]);

  // Reset state only when dialog transitions from closed to open
  // This prevents re-initialization when config changes while dialog is already open (Bug #4 & #5)
  useEffect(() => {
    const wasOpen = prevOpenRef.current;
    prevOpenRef.current = open;

    // Only initialize when transitioning from closed (false) to open (true)
    if (open && !wasOpen) {
      reset();
      resetSelection();
      setTitle('');
      setTitleModifiedByUser(false);
      setSelectedLang(selectedLanguage || 'auto');
      setShowAdvanced(false);

      resetYoutubeFlow();
      setYoutubeSubTab('single');
      setBatchInput('');
      resetBatch();
      setActiveTab('upload');

      // Validate preselected file if provided
      if (preselectedFile) {
        validateFile(preselectedFile).then((info) => {
          if (info) {
            setTitle(info.filename);
          }
        });
      }

      // Fetch available models using centralized hook
      fetchModels();
    }
  }, [open, preselectedFile, selectedLanguage, transcriptModelConfig, reset, resetSelection, resetYoutubeFlow, validateFile, fetchModels]);

  // Update title when fileInfo changes
  useEffect(() => {
    if (fileInfo && !title && !titleModifiedByUser) {
      setTitle(fileInfo.filename);
    }
  }, [fileInfo, title, titleModifiedByUser]);

  // Update editable title when a YouTube URL resolves to video info
  useEffect(() => {
    if (videoInfo && !youtubeTitle && !youtubeTitleModifiedByUser) {
      setYoutubeTitle(videoInfo.title);
    }
  }, [videoInfo, youtubeTitle, youtubeTitleModifiedByUser]);

  const selectedModel = useMemo((): ModelOption | undefined => {
    if (!selectedModelKey) return undefined;
    const colonIndex = selectedModelKey.indexOf(':');
    if (colonIndex === -1) return undefined;
    const provider = selectedModelKey.slice(0, colonIndex);
    const name = selectedModelKey.slice(colonIndex + 1);
    return availableModels.find((m) => m.provider === provider && m.name === name);
  }, [selectedModelKey, availableModels]);
  const isParakeetModel = selectedModel?.provider === 'parakeet';

  useEffect(() => {
    if (isParakeetModel && selectedLang !== 'auto') {
      setSelectedLang('auto');
    }
  }, [isParakeetModel, selectedLang]);

  const handleSelectFile = async () => {
    const info = await selectFile();
    if (info) {
      setTitle(info.filename);
    }
  };

  const handleStartImport = async () => {
    if (!fileInfo) return;

    await startImport(
      fileInfo.path,
      title || fileInfo.filename,
      isParakeetModel ? null : selectedLang === 'auto' ? null : selectedLang,
      selectedModel?.name || null,
      selectedModel?.provider || null
    );
  };

  const handleValidateYoutubeUrl = useCallback(async (url: string) => {
    const trimmed = url.trim();
    if (!trimmed) return;
    await validateUrl(trimmed);
  }, [validateUrl]);

  const handleYoutubeUrlPaste = (event: React.ClipboardEvent<HTMLInputElement>) => {
    const pasted = event.clipboardData.getData('text').trim();
    if (!pasted) return;
    setYoutubeUrl(pasted);
    handleValidateYoutubeUrl(pasted);
  };

  const handleStartYoutubeImport = async () => {
    if (!videoInfo) return;
    await startYoutubeImport(youtubeTitle || videoInfo.title);
  };

  // Combined processing flag across all tabs, used to guard dialog close/tab-switch
  const anyTabProcessing = isProcessing || youtubeIsProcessing || batchIsProcessing;

  // Values driving the header/progress/footer/cancel, scoped to whichever tab is active
  const isBatchMode = activeTab === 'youtube' && youtubeSubTab === 'batch';
  const activeStatus = isBatchMode ? batchStatus : (activeTab === 'upload' ? status : youtubeStatus);
  const activeError = isBatchMode ? batchError : (activeTab === 'upload' ? error : youtubeError);
  const activeIsProcessing = isBatchMode ? batchIsProcessing : (activeTab === 'upload' ? isProcessing : youtubeIsProcessing);
  const activeProgress = isBatchMode ? null : (activeTab === 'upload' ? progress : youtubeProgress);
  const activeReset = isBatchMode ? resetBatch : (activeTab === 'upload' ? reset : resetYoutube);
  const activeCancelImport = isBatchMode ? cancelBatch : (activeTab === 'upload' ? cancelImport : cancelYoutubeImport);
  const activeImportDisabled = isBatchMode
    ? batchValidCount === 0
    : (activeTab === 'upload' ? !fileInfo : !videoInfo);
  const handleActiveImport = isBatchMode
    ? handleStartBatch
    : (activeTab === 'upload' ? handleStartImport : handleStartYoutubeImport);

  const handleCancel = async () => {
    if (activeIsProcessing) {
      await activeCancelImport();
      toast.info('Import cancelled');
    }
    onOpenChange(false);
  };

  // Prevent closing during processing
  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen && anyTabProcessing) {
      return;
    }
    onOpenChange(newOpen);
  };

  const handleEscapeKeyDown = (event: KeyboardEvent) => {
    if (anyTabProcessing) {
      event.preventDefault();
    }
  };

  const handleInteractOutside = (event: Event) => {
    if (anyTabProcessing) {
      event.preventDefault();
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="sm:max-w-[500px]"
        onEscapeKeyDown={handleEscapeKeyDown}
        onInteractOutside={handleInteractOutside}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {activeIsProcessing ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                {isBatchMode
                  ? 'Importing YouTube Batch...'
                  : activeTab === 'upload'
                  ? 'Importing Audio...'
                  : 'Importing from YouTube...'}
              </>
            ) : activeError ? (
              <>
                <AlertCircle className="h-5 w-5 text-destructive" />
                Import Failed
              </>
            ) : activeStatus === 'complete' ? (
              <>
                <CheckCircle2 className="h-5 w-5 text-success" />
                Import Complete
              </>
            ) : activeTab === 'upload' ? (
              <>
                <Upload className="h-5 w-5 text-primary" />
                Import Audio File
              </>
            ) : isBatchMode ? (
              <>
                <ListPlus className="h-5 w-5 text-primary" />
                Batch Import from YouTube
              </>
            ) : (
              <>
                <Youtube className="h-5 w-5 text-primary" />
                Import from YouTube
              </>
            )}
          </DialogTitle>
          <DialogDescription>
            {activeIsProcessing
              ? activeProgress?.message || 'Processing...'
              : activeError
              ? 'An error occurred during import'
              : isBatchMode
              ? 'Paste one YouTube URL per line. Downloads run in parallel; transcription runs serially.'
              : 'Import an audio file or a YouTube video to create a new meeting with transcripts'}
          </DialogDescription>
        </DialogHeader>

        <Tabs
          value={activeTab}
          onValueChange={(value) => {
            if (anyTabProcessing) return;
            setActiveTab(value as ImportTab);
          }}
        >
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="upload" disabled={anyTabProcessing}>
              <Upload className="h-4 w-4 mr-2" />
              Upload File
            </TabsTrigger>
            <TabsTrigger value="youtube" disabled={anyTabProcessing}>
              <Youtube className="h-4 w-4 mr-2" />
              YouTube Link
            </TabsTrigger>
          </TabsList>

          <TabsContent value="upload" className="space-y-4 py-2">
            {!isProcessing && !error && (
              <>
              {fileInfo ? (
                <div className="glass-card p-4 space-y-3">
                  <div className="flex items-start gap-3">
                    <FileAudio className="h-8 w-8 text-primary flex-shrink-0" />
                    <div className="flex-1 min-w-0">
                      <p className="font-medium text-foreground truncate">{fileInfo.filename}</p>
                      <div className="flex items-center gap-4 text-sm text-muted-foreground mt-1">
                        <span className="flex items-center gap-1">
                          <Clock className="h-3.5 w-3.5" />
                          {formatDuration(fileInfo.duration_seconds)}
                        </span>
                        <span className="flex items-center gap-1">
                          <HardDrive className="h-3.5 w-3.5" />
                          {formatFileSize(fileInfo.size_bytes)}
                        </span>
                        <span className="text-primary font-medium">{fileInfo.format}</span>
                      </div>
                    </div>
                  </div>

                  {/* Editable title */}
                  <div className="space-y-1">
                    <label className="text-sm font-medium text-foreground/80">Meeting Title</label>
                    <Input
                      value={title}
                      onChange={(e) => {
                        setTitle(e.target.value);
                        setTitleModifiedByUser(true);
                      }}
                      placeholder="Enter meeting title"
                    />
                  </div>

                  <Button variant="outline" size="sm" onClick={handleSelectFile} className="w-full">
                    Choose Different File
                  </Button>
                </div>
              ) : (
                <div className="glass-dashed border-2 p-8 text-center">
                  <FileAudio className="h-12 w-12 text-muted-foreground/60 mx-auto mb-4" />
                  <Button onClick={handleSelectFile} disabled={status === 'validating'}>
                    {status === 'validating' ? (
                      <>
                        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                        Validating...
                      </>
                    ) : (
                      <>
                        <Upload className="h-4 w-4 mr-2" />
                        Select Audio File
                      </>
                    )}
                  </Button>
                  <p className="text-sm text-muted-foreground mt-2">MP4, WAV, MP3, FLAC, OGG, MKV, WebM, WMA</p>
                </div>
              )}

              {/* Advanced options (collapsible) */}
              {fileInfo && (
                <div className="border border-border/10 rounded-lg">
                  <button
                    onClick={() => setShowAdvanced(!showAdvanced)}
                    className="w-full flex items-center justify-between p-3 text-sm font-medium text-foreground/80 hover:bg-secondary/10"
                  >
                    <span>Advanced Options</span>
                    {showAdvanced ? (
                      <ChevronUp className="h-4 w-4" />
                    ) : (
                      <ChevronDown className="h-4 w-4" />
                    )}
                  </button>

                  {showAdvanced && (
                    <div className="p-3 pt-0 space-y-4 border-t border-border/10">
                      {/* Language selector */}
                      {!isParakeetModel ? (
                        <div className="space-y-2">
                          <div className="flex items-center gap-2">
                            <Globe className="h-4 w-4 text-muted-foreground" />
                            <span className="text-sm font-medium">Language</span>
                          </div>
                          <Select value={selectedLang} onValueChange={setSelectedLang}>
                            <SelectTrigger className="w-full">
                              <SelectValue placeholder="Select language" />
                            </SelectTrigger>
                            <SelectContent className="max-h-60">
                              {LANGUAGES.map((lang) => (
                                <SelectItem key={lang.code} value={lang.code}>
                                  {lang.name}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </div>
                      ) : (
                        <div className="space-y-2">
                          <div className="flex items-center gap-2">
                            <Globe className="h-4 w-4 text-muted-foreground" />
                            <span className="text-sm font-medium">Language</span>
                          </div>
                          <p className="text-xs text-muted-foreground">
                            Language selection isn't supported for Parakeet. It always uses automatic detection.
                          </p>
                        </div>
                      )}

                      {/* Model selector */}
                      {availableModels.length > 0 && (
                        <div className="space-y-2">
                          <div className="flex items-center gap-2">
                            <Cpu className="h-4 w-4 text-muted-foreground" />
                            <span className="text-sm font-medium">Model</span>
                          </div>
                          <Select
                            value={selectedModelKey}
                            onValueChange={setSelectedModelKey}
                            disabled={loadingModels}
                          >
                            <SelectTrigger className="w-full">
                              <SelectValue placeholder={loadingModels ? 'Loading models...' : 'Select model'} />
                            </SelectTrigger>
                            <SelectContent>
                              {availableModels.map((model) => (
                                <SelectItem
                                  key={`${model.provider}:${model.name}`}
                                  value={`${model.provider}:${model.name}`}
                                >
                                  {model.displayName} ({Math.round(model.size_mb)} MB)
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )}
              </>
            )}
          </TabsContent>

          <TabsContent value="youtube" className="space-y-4 py-2">
            <div className="flex items-center gap-2 p-1 rounded-lg bg-secondary/20 w-fit">
              <button
                type="button"
                onClick={() => setYoutubeSubTab('single')}
                disabled={batchIsProcessing}
                className={`px-3 py-1.5 text-sm font-medium rounded-md flex items-center gap-1.5 ${
                  youtubeSubTab === 'single'
                    ? 'bg-primary text-primary-foreground'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                <Youtube className="h-3.5 w-3.5" />
                Single
              </button>
              <button
                type="button"
                onClick={() => setYoutubeSubTab('batch')}
                disabled={youtubeIsProcessing}
                className={`px-3 py-1.5 text-sm font-medium rounded-md flex items-center gap-1.5 ${
                  youtubeSubTab === 'batch'
                    ? 'bg-primary text-primary-foreground'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                <ListPlus className="h-3.5 w-3.5" />
                Batch
              </button>
            </div>

            {youtubeSubTab === 'single' && !youtubeIsProcessing && !youtubeError && (
              <>
                {videoInfo ? (
                  <div className="glass-card p-4 space-y-3">
                    <div className="flex items-start gap-3">
                      {videoInfo.thumbnail_url ? (
                        // eslint-disable-next-line @next/next/no-img-element
                        <img
                          src={videoInfo.thumbnail_url}
                          alt=""
                          className="h-16 w-28 rounded object-cover flex-shrink-0"
                        />
                      ) : (
                        <Youtube className="h-8 w-8 text-primary flex-shrink-0" />
                      )}
                      <div className="flex-1 min-w-0">
                        <p className="font-medium text-foreground truncate">{videoInfo.title}</p>
                        <div className="flex items-center gap-4 text-sm text-muted-foreground mt-1">
                          {videoInfo.duration_seconds != null && (
                            <span className="flex items-center gap-1">
                              <Clock className="h-3.5 w-3.5" />
                              {formatDuration(videoInfo.duration_seconds)}
                            </span>
                          )}
                          {videoInfo.channel && (
                            <span className="truncate">{videoInfo.channel}</span>
                          )}
                        </div>
                      </div>
                    </div>

                    <div className="space-y-1">
                      <label className="text-sm font-medium text-foreground/80">Meeting Title</label>
                      <Input
                        value={youtubeTitle}
                        onChange={(e) => {
                          setYoutubeTitle(e.target.value);
                          setYoutubeTitleModifiedByUser(true);
                        }}
                        placeholder="Enter meeting title"
                      />
                    </div>

                    <Button
                      variant="outline"
                      size="sm"
                      onClick={resetYoutubeFlow}
                      className="w-full"
                    >
                      Use a Different Video
                    </Button>
                  </div>
                ) : (
                  <div className="glass-dashed border-2 p-8 text-center space-y-3">
                    <Youtube className="h-12 w-12 text-muted-foreground/60 mx-auto" />
                    <Input
                      value={youtubeUrl}
                      onChange={(e) => setYoutubeUrl(e.target.value)}
                      onPaste={handleYoutubeUrlPaste}
                      placeholder="https://www.youtube.com/watch?v=..."
                      disabled={youtubeStatus === 'validating'}
                    />
                    <Button
                      onClick={() => handleValidateYoutubeUrl(youtubeUrl)}
                      disabled={!youtubeUrl.trim() || youtubeStatus === 'validating'}
                    >
                      {youtubeStatus === 'validating' ? (
                        <>
                          <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                          Looking up video...
                        </>
                      ) : (
                        <>
                          <Globe className="h-4 w-4 mr-2" />
                          Validate
                        </>
                      )}
                    </Button>
                    <p className="text-sm text-muted-foreground">Paste a YouTube video URL to import its audio</p>
                  </div>
                )}
              </>
            )}

            {youtubeSubTab === 'batch' && !batchIsProcessing && !batchIsFinished && (
              <div className="space-y-3">
                <div className="space-y-1">
                  <label className="text-sm font-medium text-foreground/80">YouTube URLs</label>
                  <Textarea
                    value={batchInput}
                    onChange={(e) => handleBatchQueueChange(e.target.value)}
                    placeholder={'Paste YouTube URLs, one per line:\nhttps://www.youtube.com/watch?v=...\nhttps://youtu.be/...'}
                    rows={6}
                    className="font-mono text-sm"
                  />
                  <p className="text-xs text-muted-foreground">
                    {batchValidCount > 0
                      ? `${batchValidCount} valid URL${batchValidCount === 1 ? '' : 's'} ready`
                      : 'Enter at least one YouTube URL to start a batch'}
                    {batchQueue.length > batchValidCount &&
                      ` · ${batchQueue.length - batchValidCount} invalid`}
                  </p>
                </div>

                {batchQueue.length > 0 && (
                  <div className="border border-border/10 rounded-lg max-h-40 overflow-y-auto">
                    {batchQueue.map((entry, idx) => (
                      <div
                        key={`${entry.url}-${idx}`}
                        className={`flex items-center gap-2 px-3 py-1.5 text-sm ${
                          entry.valid ? '' : 'text-destructive'
                        } ${idx > 0 ? 'border-t border-border/10' : ''}`}
                      >
                        <span className="truncate flex-1 font-mono text-xs">{entry.url}</span>
                        {entry.valid ? (
                          <CheckCircle2 className="h-3.5 w-3.5 text-success flex-shrink-0" />
                        ) : (
                          <AlertCircle className="h-3.5 w-3.5 text-destructive flex-shrink-0" />
                        )}
                      </div>
                    ))}
                  </div>
                )}

                <Button
                  onClick={handleStartBatch}
                  disabled={batchValidCount === 0}
                  className="w-full bg-primary text-background hover:bg-primary/90"
                >
                  <Play className="h-4 w-4 mr-2" />
                  Start batch ({batchValidCount})
                </Button>
              </div>
            )}

            {youtubeSubTab === 'batch' && (batchIsProcessing || batchIsFinished) && (
              <div className="space-y-3">
                <div className="space-y-1">
                  <div className="flex justify-between text-sm">
                    <span className="font-medium">
                      {batchStatus === 'idle' && batchIsFinished
                        ? 'Batch cancelled'
                        : batchIsFinished
                        ? 'Batch complete'
                        : 'Processing batch'}
                    </span>
                    <span className="text-muted-foreground">
                      {batchCompleted} / {batchItems.length} · {batchFailed} failed
                    </span>
                  </div>
                  <div className="w-full bg-secondary/10 rounded-full h-2">
                    <div
                      className="bg-primary h-2 rounded-full transition-all duration-300"
                      style={{
                        width: `${batchItems.length > 0 ? (batchCompleted / batchItems.length) * 100 : 0}%`,
                      }}
                    />
                  </div>
                </div>

                <div className="border border-border/10 rounded-lg max-h-64 overflow-y-auto">
                  {batchItems.length === 0 ? (
                    <div className="p-4 text-sm text-muted-foreground text-center">
                      Waiting for first item...
                    </div>
                  ) : (
                    batchItems.map((item) => (
                      <div
                        key={item.index}
                        className={`flex items-center gap-2 px-3 py-2 text-sm ${
                          item.index > 0 ? 'border-t border-border/10' : ''
                        }`}
                      >
                        <BatchItemStatusIcon status={item.status} />
                        <div className="flex-1 min-w-0">
                          <p className="truncate font-mono text-xs">{item.url}</p>
                          {item.error && (
                            <p className="text-xs text-destructive truncate">{item.error}</p>
                          )}
                        </div>
                        <span className="text-xs text-muted-foreground flex-shrink-0">
                          {item.status === 'downloading' || item.status === 'transcribing'
                            ? `${item.progress_percentage}%`
                            : item.status}
                        </span>
                      </div>
                    ))
                  )}
                </div>

                {batchIsFinished && (
                  <Button
                    variant="outline"
                    onClick={resetBatch}
                    className="w-full"
                  >
                    <Trash2 className="h-4 w-4 mr-2" />
                    Clear queue
                  </Button>
                )}
              </div>
            )}
          </TabsContent>
        </Tabs>

        <div className="space-y-4 py-4">
          {/* Progress display (shared between the upload and YouTube flows) */}
          {activeIsProcessing && activeProgress && (
            <div className="space-y-2">
              <div className="relative">
                <div className="w-full bg-secondary/10 rounded-full h-3">
                  <div
                    className="bg-primary h-3 rounded-full transition-all duration-300 ease-out"
                    style={{ width: `${Math.min(activeProgress.progress_percentage, 100)}%` }}
                  />
                </div>
                <div className="flex justify-between text-xs text-muted-foreground mt-1">
                  <span>{activeProgress.stage}</span>
                  <span>{Math.round(activeProgress.progress_percentage)}%</span>
                </div>
              </div>
              <p className="text-sm text-muted-foreground text-center">{activeProgress.message}</p>
            </div>
          )}

          {/* Error display (shared between the upload and YouTube flows) */}
          {activeError && (
            <div className="bg-destructive/15 border border-destructive/20 rounded-lg p-3">
              <p className="text-sm text-destructive">{activeError}</p>
            </div>
          )}
        </div>

        <DialogFooter>
          {!activeIsProcessing && !activeError && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button
                onClick={handleActiveImport}
                className="bg-primary text-background hover:bg-primary/90"
                disabled={activeImportDisabled}
              >
                {activeTab === 'upload' ? (
                  <Upload className="h-4 w-4 mr-2" />
                ) : isBatchMode ? (
                  <ListPlus className="h-4 w-4 mr-2" />
                ) : (
                  <Youtube className="h-4 w-4 mr-2" />
                )}
                {isBatchMode ? 'Start batch' : 'Import'}
              </Button>
            </>
          )}
          {activeIsProcessing && (
            <Button variant="outline" onClick={handleCancel}>
              <X className="h-4 w-4 mr-2" />
              Cancel
            </Button>
          )}
          {activeError && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button onClick={activeReset} variant="outline">
                Try Again
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
