'use client';

import { useCallback, useRef, useState, type KeyboardEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';

/** One completed question/answer exchange, oldest first. */
export interface AskTurn {
  id: string;
  question: string;
  answer: string;
}

export interface UseAskAIOptions {
  /**
   * Empties the input as soon as a question is dispatched. Wanted by the
   * threaded live panel, where the asked question moves into the transcript
   * of the conversation; the single-answer panels keep it in place.
   */
  clearQuestionOnSubmit?: boolean;
}

export interface UseAskAIResult {
  question: string;
  setQuestion: (value: string) => void;
  answer: string | null;
  /** Every answered exchange this session, for panels that show a thread. */
  turns: AskTurn[];
  /** The in-flight question, so a thread can show it before the answer lands. */
  pendingQuestion: string | null;
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
 * Answered exchanges also accumulate in `turns` for panels that render the
 * whole conversation rather than just the latest answer.
 *
 * @param command Tauri command name to invoke, e.g. 'ask_about_meeting'.
 * @param buildArgs Builds the invoke() args from the trimmed question.
 * @param options Opt-in behavior; see {@link UseAskAIOptions}.
 */
export function useAskAI(
  command: string,
  buildArgs: (question: string) => Record<string, unknown>,
  options: UseAskAIOptions = {}
): UseAskAIResult {
  const [question, setQuestion] = useState('');
  const [answer, setAnswer] = useState<string | null>(null);
  const [turns, setTurns] = useState<AskTurn[]>([]);
  const [pendingQuestion, setPendingQuestion] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Read through refs rather than the closures over `question`/`buildArgs`,
  // so `ask`'s identity doesn't change every keystroke.
  const questionRef = useRef(question);
  questionRef.current = question;
  const buildArgsRef = useRef(buildArgs);
  buildArgsRef.current = buildArgs;
  const clearQuestionOnSubmit = options.clearQuestionOnSubmit ?? false;

  // Turn ids only have to be stable and unique within one mounted panel, so a
  // counter beats a timestamp/random id (and keeps tests deterministic).
  const nextTurnId = useRef(0);

  const ask = useCallback(() => {
    const trimmed = questionRef.current.trim();
    if (!trimmed || isLoading) {
      return;
    }

    setIsLoading(true);
    setError(null);
    setAnswer(null);
    setPendingQuestion(trimmed);
    if (clearQuestionOnSubmit) {
      setQuestion('');
    }

    invoke<string>(command, buildArgsRef.current(trimmed))
      .then(result => {
        setAnswer(result);
        setTurns(prev => [
          ...prev,
          { id: `turn-${nextTurnId.current++}`, question: trimmed, answer: result },
        ]);
      })
      .catch((err: unknown) => {
        const message = extractErrorMessage(err);
        setError(message || 'Failed to get an answer. Please try again.');
      })
      .finally(() => {
        setIsLoading(false);
        setPendingQuestion(null);
      });
  }, [command, isLoading, clearQuestionOnSubmit]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        ask();
      }
    },
    [ask]
  );

  const isSubmitDisabled = isLoading || !question.trim();

  return {
    question,
    setQuestion,
    answer,
    turns,
    pendingQuestion,
    isLoading,
    error,
    ask,
    handleKeyDown,
    isSubmitDisabled,
  };
}
