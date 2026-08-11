import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, renderHook, waitFor } from '@testing-library/react';
import { mockTauriInvoke } from '../components/askPanelTestUtils';

const invoke = mockTauriInvoke();

const { useAskAI } = await import('../../src/hooks/useAskAI');

function setup() {
  return renderHook(() => useAskAI('ask_about_meeting', (question) => ({ meetingId: 'm1', question })));
}

async function askAndSettle(hook: ReturnType<typeof setup>) {
  act(() => {
    hook.result.current.setQuestion('Question?');
  });
  act(() => {
    hook.result.current.ask();
  });
  await waitFor(() => expect(hook.result.current.isLoading).toBe(false));
}

describe('useAskAI extractErrorMessage adversarial rejections', () => {
  beforeEach(() => {
    invoke.reset();
  });

  afterEach(() => {
    mock.restore();
  });

  test('circular-reference rejection object does not crash the request and still surfaces the fallback message', async () => {
    // JSON.stringify on a circular object throws; extractErrorMessage's own
    // try/catch around it must swallow that and fall through to null, not
    // let the throw escape .catch() and leave the request hung.
    const circular: Record<string, unknown> = { code: 'LOOP' };
    circular.self = circular;
    invoke.setImpl(() => Promise.reject(circular));

    const hook = setup();
    await askAndSettle(hook);

    expect(hook.result.current.isLoading).toBe(false);
    expect(hook.result.current.error).toBe('Failed to get an answer. Please try again.');
  });

  test('rejection with a plain number surfaces the fallback message, not the number itself', async () => {
    invoke.setImpl(() => Promise.reject(42));

    const hook = setup();
    await askAndSettle(hook);

    expect(hook.result.current.error).toBe('Failed to get an answer. Please try again.');
  });

  test('rejection with undefined surfaces the fallback message', async () => {
    invoke.setImpl(() => Promise.reject(undefined));

    const hook = setup();
    await askAndSettle(hook);

    expect(hook.result.current.error).toBe('Failed to get an answer. Please try again.');
  });
});
