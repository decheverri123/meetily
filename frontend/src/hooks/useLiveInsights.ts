'use client';

import { useState, useEffect, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';

/** How often we poll the backend for a fresh running summary while recording. */
const POLL_INTERVAL_MS = 45000;

/**
 * Minimum growth (in transcript characters) since the last successful refresh
 * before we bother calling the backend again. Keeps us from re-generating
 * insights on every tick when nobody has said anything new.
 */
const MIN_GROWTH_CHARS = 40;

/**
 * Exact rejection reason the backend uses to signal "a call is already running".
 * Must stay byte-identical to the Rust-side constant in
 * frontend/src-tauri/src/audio/recording_commands.rs.
 */
const IN_PROGRESS_ERROR = 'insights generation already in progress';

export interface UseLiveInsightsResult {
  /** Latest markdown-formatted running summary + action items. Empty string until first content arrives. */
  insights: string;
  /** True while a generate_live_insights call is in flight. */
  isLoading: boolean;
  /** Lightweight error message from the most recent failed call, if any. Does not clear `insights`. */
  error: string | null;
}

/**
 * Polls the backend for a running summary + action items while a meeting is being recorded.
 *
 * - Only polls while recording is active.
 * - Skips calling the backend on ticks where the transcript hasn't grown enough to matter.
 * - Treats an empty-string resolution as "nothing new" and keeps the last insights visible.
 * - Silently ignores the "already in progress" rejection (no error surfaced).
 * - Keeps the last successful insights visible alongside any other error.
 * - Clears state when a new recording starts (but leaves the previous insights visible after a stop).
 * - Discards any response that resolves after a new recording has already started (request-epoch guard),
 *   so a stale cross-meeting response can never overwrite the new meeting's insights/error/growth-gate state.
 */
export function useLiveInsights(): UseLiveInsightsResult {
  const { isRecording } = useRecordingState();
  const { transcriptsRef } = useTranscripts();

  const [insights, setInsights] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const lastLengthRef = useRef(0);
  const isFetchingRef = useRef(false);
  // Lazily seeded from the CURRENT isRecording value (not hardcoded `false`):
  // this hook/component can mount mid-recording (e.g. the user toggles the
  // Live Insights panel back on), and without this the mount would look like
  // a false → true "new recording started" transition, wrongly resetting
  // lastLengthRef/insights/error as if this were a fresh meeting.
  const wasRecordingRef = useRef(isRecording);
  // Bumped on every observed false → true recording transition. Lets an
  // in-flight generate_live_insights call recognize, once it resolves, that
  // it was fired for a since-ended recording session and should be discarded
  // instead of being applied to the new session's state.
  const epochRef = useRef(0);

  const getTranscriptLength = useCallback(() => {
    return transcriptsRef.current.reduce((total, t) => total + (t.text?.length ?? 0), 0);
  }, [transcriptsRef]);

  const refresh = useCallback(async () => {
    if (isFetchingRef.current) return;

    const currentLength = getTranscriptLength();
    if (currentLength - lastLengthRef.current < MIN_GROWTH_CHARS) {
      return;
    }

    isFetchingRef.current = true;
    setIsLoading(true);
    const requestEpoch = epochRef.current;

    try {
      const result = await invoke<string>('generate_live_insights');

      // The recording that requested this call may have already ended (and a
      // new one started) while the call was in flight. If so, this response
      // belongs to a prior meeting - discard it entirely rather than letting
      // it overwrite the new meeting's state (including lastLengthRef, which
      // would otherwise silently suppress updates for the whole new meeting).
      if (requestEpoch !== epochRef.current) {
        return;
      }

      // Call succeeded (even if empty) - this is now our "last successful refresh" point.
      lastLengthRef.current = currentLength;
      setError(null);

      if (result.trim().length > 0) {
        setInsights(result);
      }
      // Empty string = "no update" - intentionally keep any previous insights displayed.
    } catch (err) {
      if (requestEpoch !== epochRef.current) {
        return;
      }

      const message = err instanceof Error ? err.message : String(err);

      if (message === IN_PROGRESS_ERROR) {
        // Skip silently: a previous call is still running, try again next tick.
      } else {
        setError(message || 'Failed to generate live insights');
      }
    } finally {
      isFetchingRef.current = false;
      setIsLoading(false);
    }
  }, [getTranscriptLength]);

  // Reset state when a brand new recording starts, so a previous meeting's
  // insights don't linger into the next one. Left untouched on stop so the
  // user can keep reading it after the meeting ends.
  useEffect(() => {
    if (isRecording && !wasRecordingRef.current) {
      setInsights('');
      setError(null);
      lastLengthRef.current = 0;
      // New recording session: any in-flight (or not-yet-fired) call from
      // before this point belongs to the old session. Bumping the epoch lets
      // `refresh` recognize and discard such a response when it resolves.
      epochRef.current += 1;
    }
    wasRecordingRef.current = isRecording;
  }, [isRecording]);

  // Poll only while recording is active.
  useEffect(() => {
    if (!isRecording) {
      return;
    }

    const intervalId = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(intervalId);
  }, [isRecording, refresh]);

  return { insights, isLoading, error };
}
