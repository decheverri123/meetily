'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import {
  LIVE_GENERATION_IN_PROGRESS_ERROR,
  LIVE_GENERATION_RATE_LIMITED_ERROR,
} from '@/lib/liveGenerationErrors';

/** The two supported chip kinds, matching the Rust-side `kind` values exactly. */
export type LiveActionChipKind = 'recap' | 'questions';

export interface LiveActionChipState {
  /**
   * Latest markdown-formatted chip result. Empty string until first content
   * arrives, and normalized to '' if the backend resolved with a
   * whitespace-only string (see `generate`'s success handler).
   */
  result: string;
  /** True while a generate_live_action_chip call for this kind is in flight. */
  isLoading: boolean;
  /** Error message from the most recent failed call, if any. */
  error: string | null;
  /**
   * True when `error` is one of the two retry-friendly sentinel rejections
   * (single-flight lock busy or rate-limited) rather than a real failure -
   * lets the UI show a softer "busy, try again" tone instead of an error state.
   */
  isRetryable: boolean;
  /**
   * True once a `generate_live_action_chip` call for this kind has resolved
   * successfully at least once - even if it resolved with `Ok("")` (not
   * enough transcript yet, see `LIVE_INSIGHTS_MIN_CHARS`). Distinguishes
   * "clicked but got nothing useful back" from "never clicked", since both
   * cases otherwise leave `result` as an empty string. Reset to `false`
   * whenever chip state is reset for a new recording.
   */
  hasGenerated: boolean;
}

const INITIAL_CHIP_STATE: LiveActionChipState = {
  result: '',
  isLoading: false,
  error: null,
  isRetryable: false,
  hasGenerated: false,
};

export interface UseLiveActionChipsResult {
  /** Per-kind state, independent between "recap" and "questions". */
  chips: Record<LiveActionChipKind, LiveActionChipState>;
  /**
   * Trigger generation for the given kind. Safe to call while a previous call
   * for the same kind is still loading (backend will reject as in-progress).
   * No-ops once recording has stopped (see `isRecording`).
   */
  generate: (kind: LiveActionChipKind) => void;
  /** True if any chip has a result, error, or in-flight request - used by callers to decide whether to keep the chips mounted after recording stops. */
  hasActivity: boolean;
  /**
   * Mirrors `useRecordingState().isRecording`. Exposed so callers (the chip
   * buttons) can disable new generation once recording stops - `stop_recording`
   * clears the Rust-side recording state, so a post-stop call would resolve
   * with an empty transcript window and silently overwrite a good result with
   * "not enough conversation yet".
   */
  isRecording: boolean;
}

/**
 * On-demand generator for the "Recap" and "Questions to ask" action chips
 * shown during live recording.
 *
 * Mirrors useLiveInsights' patterns:
 * - Discards any response that resolves after a new recording has already
 *   started (request-epoch guard), so a stale cross-meeting response can
 *   never overwrite the new meeting's chip state.
 * - Clears state when a new recording starts (but leaves results visible
 *   after a stop, so the user can keep reading a recap after ending).
 * - Distinguishes the two retry-friendly sentinel errors from real failures.
 *
 * Unlike useLiveInsights, generation is triggered explicitly by the user
 * (button click) rather than polled, and each kind tracks fully independent
 * state so clicking "Recap" can never clobber "Questions" state or vice versa.
 */
export function useLiveActionChips(): UseLiveActionChipsResult {
  const { isRecording } = useRecordingState();

  const [chips, setChips] = useState<Record<LiveActionChipKind, LiveActionChipState>>({
    recap: INITIAL_CHIP_STATE,
    questions: INITIAL_CHIP_STATE,
  });

  const wasRecordingRef = useRef(isRecording);
  // Bumped on every observed false -> true recording transition. Lets an
  // in-flight generate_live_action_chip call recognize, once it resolves,
  // that it was fired for a since-ended recording session and should be
  // discarded instead of being applied to the new session's state.
  const epochRef = useRef(0);

  const generate = useCallback((kind: LiveActionChipKind) => {
    if (!isRecording) {
      // Defense in depth alongside the disabled button - see the `isRecording`
      // doc on UseLiveActionChipsResult above for why a post-stop call must not run.
      return;
    }

    const requestEpoch = epochRef.current;

    setChips(prev => ({
      ...prev,
      [kind]: { ...prev[kind], isLoading: true, error: null, isRetryable: false },
    }));

    invoke<string>('generate_live_action_chip', { kind })
      .then(result => {
        if (requestEpoch !== epochRef.current) {
          // Belongs to a prior meeting - discard rather than overwrite the new one's state.
          return;
        }
        // Matches useLiveInsights.ts's `result.trim().length > 0` gate -
        // without it, a whitespace-only response would render as a blank
        // popover instead of the "not enough conversation yet" message.
        const normalizedResult = result.trim().length > 0 ? result : '';
        setChips(prev => ({
          ...prev,
          [kind]: {
            result: normalizedResult,
            isLoading: false,
            error: null,
            isRetryable: false,
            hasGenerated: true,
          },
        }));
      })
      .catch((err: unknown) => {
        if (requestEpoch !== epochRef.current) {
          return;
        }
        const message = err instanceof Error ? err.message : String(err);
        const isRetryable =
          message === LIVE_GENERATION_IN_PROGRESS_ERROR || message === LIVE_GENERATION_RATE_LIMITED_ERROR;

        setChips(prev => ({
          ...prev,
          [kind]: {
            ...prev[kind],
            isLoading: false,
            error: isRetryable
              ? "Still busy generating insights - try again in a moment."
              : message || 'Failed to generate chip content.',
            isRetryable,
          },
        }));
      });
  }, [isRecording]);

  // Reset state when a brand new recording starts, so a previous meeting's
  // chip results don't linger into the next one. Left untouched on stop so
  // the user can keep reading it after the meeting ends.
  useEffect(() => {
    if (isRecording && !wasRecordingRef.current) {
      setChips({ recap: INITIAL_CHIP_STATE, questions: INITIAL_CHIP_STATE });
      epochRef.current += 1;
    }
    wasRecordingRef.current = isRecording;
  }, [isRecording]);

  const hasActivity = useMemo(
    () => Object.values(chips).some(chip => chip.result || chip.error || chip.isLoading),
    [chips]
  );

  return { chips, generate, hasActivity, isRecording };
}
