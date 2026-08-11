'use client';

import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/**
 * Questions worth asking about *this* meeting, generated from its own
 * transcript by `suggest_live_transcript_questions` /
 * `suggest_meeting_questions`. Returns nothing until generation actually
 * produces something usable - a canned generic fallback reads as the same
 * three chips on every meeting, which is worse than no chips at all.
 */

const MAX_SUGGESTIONS = 3;
/** Long enough to be a real question, short enough to fit a chip. */
const MAX_SUGGESTION_LENGTH = 90;

/**
 * Pulls chip-sized questions out of an LLM reply. The prompt asks for bare
 * one-per-line questions, but models routinely add bullets, numbering, or a
 * "Here are three questions:" preamble anyway, so each line is stripped of
 * list markers and surrounding quotes and then kept only if it still looks
 * like a question. Exported for tests.
 */
export function parseSuggestedQuestions(reply: string): string[] {
  const seen = new Set<string>();

  return reply
    .split('\n')
    .map(line => line.replace(/^\s*(?:[-*•]|\d+[.)])\s*/, '').trim())
    .map(line => line.replace(/^["'"']|["'"']$/g, '').trim())
    .filter(line => {
      if (!line.endsWith('?') || line.length > MAX_SUGGESTION_LENGTH) return false;
      const key = line.toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .slice(0, MAX_SUGGESTIONS);
}

/**
 * Suggestions already generated this session, so re-opening a meeting (or
 * toggling the panel shut and back open) doesn't re-bill a paid provider for
 * an answer that hasn't changed. Module-level and unbounded on purpose: one
 * short array per meeting per refresh bucket, for the life of the window.
 */
const cache = new Map<string, string[]>();

const EMPTY_SUGGESTIONS: string[] = [];

export function useSuggestedQuestions({
  command,
  args,
  scope,
  enabled = true,
  refreshKey,
}: {
  command: string;
  /** invoke() args. Identity is ignored; `refreshKey` drives regeneration. */
  args: Record<string, unknown>;
  /** What these suggestions are about, e.g. a meeting id. Caches per scope. */
  scope: string;
  /** False while there is nothing to generate from yet (e.g. before the first words of a live meeting). */
  enabled?: boolean;
  /**
   * Change this to regenerate. A live meeting keeps talking long after the
   * first suggestions land, so callers bucket transcript growth into this
   * rather than leaving minute-two suggestions up for the whole meeting.
   */
  refreshKey?: string | number;
}): readonly string[] {
  const cacheKey = `${command}|${scope}|${refreshKey ?? ''}`;
  const [generated, setGenerated] = useState<string[] | null>(
    () => cache.get(cacheKey) ?? null
  );

  // Read args through a ref so a caller re-creating the object every render
  // can't retrigger generation - only `enabled` and `refreshKey` do.
  const argsRef = useRef(args);
  argsRef.current = args;

  useEffect(() => {
    if (!enabled) return;

    const cached = cache.get(cacheKey);
    if (cached) {
      setGenerated(cached);
      return;
    }

    let cancelled = false;
    invoke<string>(command, argsRef.current)
      .then(reply => {
        const questions = parseSuggestedQuestions(reply);
        if (questions.length === 0) return;
        cache.set(cacheKey, questions);
        if (!cancelled) {
          setGenerated(questions);
        }
      })
      .catch(() => {
        // Suggestions are a convenience; a failure here should never surface
        // as an error state - the composer just shows no chips.
      });

    return () => {
      cancelled = true;
    };
  }, [command, cacheKey, enabled]);

  return generated ?? EMPTY_SUGGESTIONS;
}
