'use client';

import { useCallback, useRef, useState, type KeyboardEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface UseAskAIResult {
  question: string;
  setQuestion: (value: string) => void;
  answer: string | null;
  isLoading: boolean;
  error: string | null;
  /** No-ops while a request is already in flight or the question is blank. */
  ask: () => void;
  /** Submits on Enter; shared so both ask panels bind the same key behavior. */
  handleKeyDown: (e: KeyboardEvent<HTMLInputElement>) => void;
  /** True while loading or the question is blank/whitespace-only. */
  isSubmitDisabled: boolean;
}

/**
 * Extracts a human-readable message from an `invoke()` rejection. Tauri
 * commands normally reject with a plain string (the Rust `Err(String)`), but
 * anything JS can throw/reject with is possible in principle. A plain object
 * would otherwise stringify via `Object.prototype.toString` to the useless
 * "[object Object]", so this pulls out a `.message` field or falls back to
 * JSON before giving up.
 */
function extractErrorMessage(err: unknown): string | null {
  if (err instanceof Error) {
    return err.message;
  }
  if (typeof err === 'string') {
    return err;
  }
  if (err && typeof err === 'object') {
    const maybeMessage = (err as Record<string, unknown>).message;
    if (typeof maybeMessage === 'string' && maybeMessage.trim()) {
      return maybeMessage;
    }
    try {
      return JSON.stringify(err);
    } catch {
      return null;
    }
  }
  return null;
}

/**
 * Shared request/response state machine for "ask about this meeting" and
 * "ask across all meetings": both invoke a single-shot Tauri command that
 * resolves once with the answer text (not streamed/polled), so there's only
 * idle / loading / answer / error to track.
 *
 * @param command Tauri command name to invoke, e.g. 'ask_about_meeting'.
 * @param buildArgs Builds the invoke() args from the trimmed question.
 */
export function useAskAI(
  command: string,
  buildArgs: (question: string) => Record<string, unknown>
): UseAskAIResult {
  const [question, setQuestion] = useState('');
  const [answer, setAnswer] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Read through refs rather than the closures over `question`/`buildArgs`,
  // so `ask`'s identity doesn't change every keystroke.
  const questionRef = useRef(question);
  questionRef.current = question;
  const buildArgsRef = useRef(buildArgs);
  buildArgsRef.current = buildArgs;

  const ask = useCallback(() => {
    const trimmed = questionRef.current.trim();
    if (!trimmed || isLoading) {
      return;
    }

    setIsLoading(true);
    setError(null);
    setAnswer(null);

    invoke<string>(command, buildArgsRef.current(trimmed))
      .then(result => {
        setAnswer(result);
      })
      .catch((err: unknown) => {
        const message = extractErrorMessage(err);
        setError(message || 'Failed to get an answer. Please try again.');
      })
      .finally(() => {
        setIsLoading(false);
      });
  }, [command, isLoading]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        ask();
      }
    },
    [ask]
  );

  const isSubmitDisabled = isLoading || !question.trim();

  return { question, setQuestion, answer, isLoading, error, ask, handleKeyDown, isSubmitDisabled };
}
