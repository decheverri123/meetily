import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, renderHook, waitFor } from '@testing-library/react';

// ---------------------------------------------------------------------------
// Mocks - must be registered before importing the hook under test, since
// `useLiveActionChips.ts` imports these modules at the top level.
// ---------------------------------------------------------------------------

let mockIsRecording = false;
mock.module('@/contexts/RecordingStateContext', () => ({
  useRecordingState: () => ({ isRecording: mockIsRecording }),
}));

let invokeImpl: (cmd: string, args?: unknown) => Promise<string> = async () => '';
mock.module('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeImpl(cmd, args),
}));

const { useLiveActionChips } = await import('../../src/hooks/useLiveActionChips');

/** Bit-for-bit the module-private `INITIAL_CHIP_STATE` in useLiveActionChips.ts. */
const PRISTINE_NEVER_CLICKED_STATE = {
  result: '',
  isLoading: false,
  error: null,
  isRetryable: false,
  hasGenerated: false,
};

describe('useLiveActionChips', () => {
  beforeEach(() => {
    mockIsRecording = false;
    invokeImpl = async () => '';
  });

  afterEach(() => {
    mock.restore();
  });

  test('a real user click that resolves with "" (not enough transcript yet) must be observably different from never having clicked at all', async () => {
    // Simulate the real backend behavior for `generate_live_action_chip`:
    // `Ok("")` when there isn't enough transcript yet (see
    // `generate_bounded_live_llm_text` / LIVE_INSIGHTS_MIN_CHARS in
    // recording_commands.rs).
    let resolveInvoke!: (v: string) => void;
    invokeImpl = () => new Promise<string>(resolve => { resolveInvoke = resolve; });
    // generate() no-ops unless recording is active (see useLiveActionChips.ts) -
    // this test is about the empty-result feedback gap, not the recording guard.
    mockIsRecording = true;

    const { result } = renderHook(() => useLiveActionChips());

    act(() => {
      result.current.generate('recap');
    });

    // Immediately after clicking, isLoading should be true - a real click did happen.
    expect(result.current.chips.recap.isLoading).toBe(true);

    await act(async () => {
      resolveInvoke('');
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(result.current.chips.recap.isLoading).toBe(false);
    });

    // EXPECTED: a click that actually completed (even with nothing useful to
    // show, e.g. "not enough conversation yet") must leave SOME trace that
    // distinguishes it from the pristine pre-click state, so the popover can
    // render something other than the exact same "Click ... to generate."
    // prompt the user just clicked.
    // ACTUAL: the resulting state is bit-for-bit identical to
    // `INITIAL_CHIP_STATE` (never-clicked), so `LiveActionChips.tsx`'s
    // `!isLoading && !error && !result` branch renders the untouched
    // "Click ... to generate." placeholder again - the user gets zero
    // feedback that their click did anything.
    expect(result.current.chips.recap).not.toEqual(PRISTINE_NEVER_CLICKED_STATE);
  });

  test('a chip request in flight when a new recording starts is discarded (epoch guard), even though meeting A stopped before meeting B started', async () => {
    // Reproduces the case from useLiveActionChips' own doc comment: click a
    // chip mid-meeting-A, meeting A stops, then a brand new meeting B starts
    // before the invoke resolves. The stale response for meeting A must never
    // land in meeting B's state.
    let resolveInvoke!: (v: string) => void;
    invokeImpl = () => new Promise<string>(resolve => { resolveInvoke = resolve; });
    // generate() no-ops unless recording is active, so the click itself must
    // happen while meeting A is still recording.
    mockIsRecording = true;

    const { result, rerender } = renderHook(() => useLiveActionChips());

    act(() => {
      result.current.generate('recap');
    });
    expect(result.current.chips.recap.isLoading).toBe(true);

    // Meeting A stops, then meeting B starts (false -> true transition) while
    // the request for meeting A's epoch is still in flight.
    mockIsRecording = false;
    act(() => {
      rerender();
    });
    mockIsRecording = true;
    act(() => {
      rerender();
    });

    // Now resolve the stale request with content that would be wrong to show
    // for the new meeting.
    await act(async () => {
      resolveInvoke('STALE CONTENT FROM PRIOR EPOCH');
      await Promise.resolve();
      await Promise.resolve();
    });

    // Expected (and, per this test passing, actual): discarded. The epoch
    // bump resets state synchronously, and the stale resolution must not
    // resurrect a loading state or content. Kept here as a positive
    // regression test - the epoch logic in useLiveActionChips.ts is correct
    // for this scenario, unlike the empty-result feedback gap above.
    expect(result.current.chips.recap.result).toBe('');
    expect(result.current.chips.recap.isLoading).toBe(false);
  });
});
