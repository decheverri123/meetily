import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';

export type BatchItemStatus =
  | 'pending'
  | 'downloading'
  | 'downloaded'
  | 'transcribing'
  | 'complete'
  | 'failed'
  | 'cancelled';

export interface BatchItem {
  index: number;
  url: string;
  title: string | null;
  status: BatchItemStatus;
  progress_percentage: number;
  meeting_id: string | null;
  error: string | null;
}

export interface BatchImportStatus {
  id: string;
  total: number;
  completed: number;
  failed: number;
  items: BatchItem[];
  finished: boolean;
  cancelled: boolean;
}

export interface YoutubeBatchProgressEvent {
  id: string;
  total: number;
  completed: number;
  failed: number;
  item: BatchItem;
  finished: boolean;
  cancelled: boolean;
}

export interface YoutubeBatchCompleteEvent {
  id: string;
  total: number;
  completed: number;
  failed: number;
  cancelled: boolean;
}

export interface QueueEntry {
  url: string;
  title: string;
  valid: boolean;
  error: string | null;
}

export interface UseYoutubeBatchImportReturn {
  rawInput: string;
  setRawInput: (s: string) => void;
  queue: QueueEntry[];
  setQueue: (entries: QueueEntry[]) => void;
  status: 'idle' | 'processing' | 'complete' | 'error';
  batchId: string | null;
  items: BatchItem[];
  completed: number;
  failed: number;
  isProcessing: boolean;
  isFinished: boolean;
  error: string | null;
  startBatch: (titles?: (string | null)[]) => Promise<void>;
  cancelBatch: () => Promise<void>;
  reset: () => void;
  parseQueue: typeof parseQueueInput;
}

const YOUTUBE_URL_RE = /^(https?:\/\/)?(www\.|m\.|music\.)?(youtube\.com\/(watch\?v=[^&\s]+|shorts\/[A-Za-z0-9_-]+|embed\/[A-Za-z0-9_-]+|live\/[A-Za-z0-9_-]+)|youtu\.be\/[A-Za-z0-9_-]+)\/?(\S*)$/;

export function isLikelyYoutubeUrl(input: string): boolean {
  return YOUTUBE_URL_RE.test(input.trim());
}

export function parseQueueInput(text: string): QueueEntry[] {
  const seen = new Set<string>();
  const entries: QueueEntry[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const trimmed = raw.trim();
    if (!trimmed) continue;
    if (seen.has(trimmed)) continue;
    seen.add(trimmed);
    entries.push({
      url: trimmed,
      title: '',
      valid: isLikelyYoutubeUrl(trimmed),
      error: isLikelyYoutubeUrl(trimmed) ? null : 'Not a valid YouTube URL',
    });
  }
  return entries;
}

export function useYoutubeBatchImport(): UseYoutubeBatchImportReturn {
  const [rawInput, setRawInput] = useState('');
  const [queue, setQueue] = useState<QueueEntry[]>([]);
  const [status, setStatus] = useState<'idle' | 'processing' | 'complete' | 'error'>('idle');
  const [batchId, setBatchId] = useState<string | null>(null);
  const [items, setItems] = useState<BatchItem[]>([]);
  const [completed, setCompleted] = useState(0);
  const [failed, setFailed] = useState(0);
  const [isFinished, setIsFinished] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isCancelledRef = useRef(false);

  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setup = async () => {
      const unlistenProgress = await listen<YoutubeBatchProgressEvent>(
        'youtube-batch-progress',
        (event) => {
          if (isCancelledRef.current) return;
          setBatchId(event.payload.id);
          setItems((prev) => {
            const next = [...prev];
            const idx = event.payload.item.index;
            if (idx >= 0 && idx < next.length) {
              next[idx] = event.payload.item;
            } else if (idx >= next.length) {
              while (next.length <= idx) next.push(event.payload.item);
            }
            return next;
          });
          setCompleted(event.payload.completed);
          setFailed(event.payload.failed);
          if (event.payload.finished) {
            setIsFinished(true);
            setStatus(event.payload.cancelled ? 'idle' : 'complete');
          } else {
            setStatus('processing');
          }
        }
      );
      if (cleanedUpRef.current) {
        unlistenProgress();
        return;
      }
      unlisteners.push(unlistenProgress);

      const unlistenComplete = await listen<YoutubeBatchCompleteEvent>(
        'youtube-batch-complete',
        (event) => {
          if (isCancelledRef.current) return;
          setBatchId(event.payload.id);
          setCompleted(event.payload.completed);
          setFailed(event.payload.failed);
          setIsFinished(true);
          setStatus(event.payload.cancelled ? 'idle' : 'complete');
        }
      );
      if (cleanedUpRef.current) {
        unlistenComplete();
        unlisteners.forEach((u) => u());
        return;
      }
      unlisteners.push(unlistenComplete);
    };

    setup();

    return () => {
      cleanedUpRef.current = true;
      unlisteners.forEach((u) => u());
    };
  }, []);

  const startBatch = useCallback(
    async (titles?: (string | null)[]) => {
      isCancelledRef.current = false;
      setError(null);
      setStatus('processing');
      setIsFinished(false);
      setCompleted(0);
      setFailed(0);
      setItems([]);

      const urls = queue.filter((q) => q.valid).map((q) => q.url);
      if (urls.length === 0) {
        const msg = 'No valid YouTube URLs in queue';
        setStatus('error');
        setError(msg);
        return;
      }

      try {
        const id = await invoke<string>('start_youtube_batch_import_command', {
          urls,
          titles: titles ?? null,
        });
        setBatchId(id);
      } catch (err: any) {
        const msg = typeof err === 'string' ? err : err?.message || String(err) || 'Failed to start YouTube batch import';
        setStatus('error');
        setError(msg);
      }
    },
    [queue]
  );

  const cancelBatch = useCallback(async () => {
    isCancelledRef.current = true;
    try {
      await invoke('cancel_youtube_batch_import_command');
    } catch (err) {
      console.error('Failed to cancel YouTube batch import:', err);
    }
    setStatus('idle');
    setIsFinished(true);
  }, []);

  const reset = useCallback(() => {
    isCancelledRef.current = false;
    setRawInput('');
    setQueue([]);
    setStatus('idle');
    setBatchId(null);
    setItems([]);
    setCompleted(0);
    setFailed(0);
    setIsFinished(false);
    setError(null);
  }, []);

  return {
    rawInput,
    setRawInput,
    queue,
    setQueue,
    status,
    batchId,
    items,
    completed,
    failed,
    isProcessing: status === 'processing',
    isFinished,
    error,
    startBatch,
    cancelBatch,
    reset,
    parseQueue: parseQueueInput,
  };
}
