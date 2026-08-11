import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import Analytics from '@/lib/analytics';
import { applyPinnedSummaryLanguageToMeeting } from '@/lib/summary-language-preferences';
import { toast } from 'sonner';

export interface YoutubeVideoInfo {
  title: string;
  duration_seconds: number | null;
  channel: string | null;
  thumbnail_url: string | null;
}

export interface YoutubeImportProgress {
  stage: string;
  progress_percentage: number;
  message: string;
}

export interface YoutubeImportResult {
  meeting_id: string;
  title: string;
  segments_count: number;
  duration_seconds: number;
}

// Reuses the same shape as the local-file import's `import-error` event
// (`ImportError { error: String }` in audio/import.rs) — youtube_import.rs
// emits that same Rust struct under the `youtube-import-error` event name.
export interface YoutubeImportError {
  error: string;
}

export type YoutubeImportStatus = 'idle' | 'validating' | 'processing' | 'complete' | 'error';

export interface UseYoutubeImportOptions {
  onComplete?: (result: YoutubeImportResult) => void;
  onError?: (error: string) => void;
}

export interface UseYoutubeImportReturn {
  status: YoutubeImportStatus;
  videoInfo: YoutubeVideoInfo | null;
  progress: YoutubeImportProgress | null;
  error: string | null;
  isProcessing: boolean;
  isBusy: boolean;
  validateUrl: (url: string) => Promise<YoutubeVideoInfo | null>;
  startImport: (title?: string | null) => Promise<void>;
  cancelImport: () => Promise<void>;
  reset: () => void;
}

export function useYoutubeImport({
  onComplete,
  onError,
}: UseYoutubeImportOptions = {}): UseYoutubeImportReturn {
  const [status, setStatus] = useState<YoutubeImportStatus>('idle');
  const [videoInfo, setVideoInfo] = useState<YoutubeVideoInfo | null>(null);
  const [progress, setProgress] = useState<YoutubeImportProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Stable refs for callbacks to avoid listener re-registration on every render
  const onCompleteRef = useRef(onComplete);
  const onErrorRef = useRef(onError);
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);
  useEffect(() => { onErrorRef.current = onError; }, [onError]);

  // Cancellation guard: prevents late events from updating state after cancel
  const isCancelledRef = useRef(false);

  // The last URL passed to validateUrl(). startImport() is title-only per the
  // Tauri contract, so the validated URL has to be remembered here rather
  // than re-supplied by the caller.
  const validatedUrlRef = useRef<string | null>(null);

  // Set up event listeners (registered once, use refs for callbacks)
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setupListeners = async () => {
      // Progress events
      const unlistenProgress = await listen<YoutubeImportProgress>(
        'youtube-import-progress',
        (event) => {
          if (isCancelledRef.current) return;
          setProgress(event.payload);
          setStatus('processing');
        }
      );
      if (cleanedUpRef.current) {
        unlistenProgress();
        return;
      }
      unlisteners.push(unlistenProgress);

      // Completion event
      const unlistenComplete = await listen<YoutubeImportResult>(
        'youtube-import-complete',
        async (event) => {
          if (isCancelledRef.current) return;

          await Analytics.track('import_youtube_completed', {
            success: 'true',
            duration_seconds: event.payload.duration_seconds.toString(),
            segments_count: event.payload.segments_count.toString()
          });

          setStatus('complete');
          setProgress(null);
          try {
            await applyPinnedSummaryLanguageToMeeting(event.payload.meeting_id);
          } catch (error) {
            console.warn('Failed to apply pinned summary language to imported meeting:', error);
            toast.warning('Could not apply default summary language', {
              description: 'The imported meeting was saved, but the default summary language was not applied.',
            });
          }
          onCompleteRef.current?.(event.payload);
        }
      );
      if (cleanedUpRef.current) {
        unlistenComplete();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenComplete);

      // Error event
      const unlistenError = await listen<YoutubeImportError>(
        'youtube-import-error',
        async (event) => {
          if (isCancelledRef.current) return;

          await Analytics.trackError('import_youtube_failed', event.payload.error);

          setStatus('error');
          setError(event.payload.error);
          onErrorRef.current?.(event.payload.error);
        }
      );
      if (cleanedUpRef.current) {
        unlistenError();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenError);
    };

    setupListeners();

    return () => {
      cleanedUpRef.current = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  // If the dialog is reopened while a previously-started import is still
  // running in the background, resume reflecting its progress instead of
  // showing a stale idle state.
  useEffect(() => {
    let cancelled = false;
    invoke<boolean>('is_youtube_import_in_progress_command')
      .then((inProgress) => {
        if (!cancelled && inProgress) {
          setStatus('processing');
        }
      })
      .catch((err) => {
        console.error('Failed to check YouTube import status:', err);
      });
    return () => { cancelled = true; };
  }, []);

  // Validate a YouTube URL and fetch its video info
  const validateUrl = useCallback(async (url: string): Promise<YoutubeVideoInfo | null> => {
    setStatus('validating');
    setError(null);

    try {
      const result = await invoke<YoutubeVideoInfo>('validate_youtube_url_command', { url });
      validatedUrlRef.current = url;
      setVideoInfo(result);
      setStatus('idle');
      return result;
    } catch (err: any) {
      setStatus('error');
      const errorMsg = typeof err === 'string' ? err : (err?.message || String(err) || 'Failed to validate YouTube URL');
      setError(errorMsg);
      onErrorRef.current?.(errorMsg);
      return null;
    }
  }, []);

  // Start the import process for the most recently validated URL
  const startImport = useCallback(
    async (title?: string | null) => {
      const url = validatedUrlRef.current;
      if (!url) {
        const errorMsg = 'No YouTube URL has been validated yet';
        setStatus('error');
        setError(errorMsg);
        onErrorRef.current?.(errorMsg);
        return;
      }

      isCancelledRef.current = false;
      setStatus('processing');
      setError(null);
      setProgress(null);

      try {
        await Analytics.track('import_youtube_started', {
          duration_seconds: (videoInfo?.duration_seconds ?? '').toString(),
        });

        await invoke('start_youtube_import_command', {
          url,
          title: title || null,
        });
      } catch (err: any) {
        setStatus('error');
        const errorMsg = typeof err === 'string' ? err : (err?.message || String(err) || 'Failed to start YouTube import');
        setError(errorMsg);

        await Analytics.trackError('import_youtube_failed', errorMsg);

        onErrorRef.current?.(errorMsg);
      }
    },
    [videoInfo]
  );

  // Cancel ongoing import
  const cancelImport = useCallback(async () => {
    isCancelledRef.current = true;
    try {
      await invoke('cancel_youtube_import_command');
      setStatus('idle');
      setProgress(null);
    } catch (err: any) {
      console.error('Failed to cancel YouTube import:', err);
    }
  }, []);

  // Reset all state
  const reset = useCallback(() => {
    isCancelledRef.current = false;
    validatedUrlRef.current = null;
    setStatus('idle');
    setVideoInfo(null);
    setProgress(null);
    setError(null);
  }, []);

  return {
    status,
    videoInfo,
    progress,
    error,
    isProcessing: status === 'processing',
    isBusy: status === 'processing' || status === 'validating',
    validateUrl,
    startImport,
    cancelImport,
    reset,
  };
}
