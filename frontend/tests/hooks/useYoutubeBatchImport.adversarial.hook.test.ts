// Adversarial tests for the useYoutubeBatchImport hook itself.
// Uses mockTauriInvoke to intercept invoke(), and a fake listen() for
// Tauri events. Focus: edge cases in startBatch, cancelBatch, reset.
import { afterEach, describe, expect, test, mock } from 'bun:test';
import { cleanup, renderHook, act, waitFor } from '@testing-library/react';
import { mockTauriInvoke } from '../components/askPanelTestUtils';

// Fake event listener: collects registered handlers, lets the test
// dispatch them on demand.
type EventHandler<T = unknown> = (event: { payload: T }) => void;
const registeredListeners: { event: string; handler: EventHandler<any> }[] = [];

mock.module('@tauri-apps/api/event', () => ({
  listen: async <T,>(event: string, handler: EventHandler<T>) => {
    registeredListeners.push({ event, handler });
    return () => {
      const idx = registeredListeners.findIndex(
        (l) => l.event === event && l.handler === handler,
      );
      if (idx >= 0) registeredListeners.splice(idx, 1);
    };
  },
}));

const invoke = mockTauriInvoke();

const { useYoutubeBatchImport } = await import(
  '../../src/hooks/useYoutubeBatchImport'
);

function clearListeners() {
  registeredListeners.length = 0;
}

function fireEvent<T>(event: string, payload: T) {
  const listener = registeredListeners.find((l) => l.event === event);
  if (listener) listener.handler({ payload });
}

async function waitForListeners() {
  // The hook registers listeners in a useEffect, asynchronously.
  await waitFor(() =>
    expect(
      registeredListeners.some((l) => l.event === 'youtube-batch-progress'),
    ).toBe(true),
  );
}

describe('useYoutubeBatchImport — adversarial', () => {
  afterEach(() => {
    invoke.reset();
    clearListeners();
    cleanup();
  });

  test('startBatch with empty queue sets error state, no invoke', async () => {
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    await act(async () => {
      await result.current.startBatch();
    });

    expect(result.current.status).toBe('error');
    expect(result.current.error).toContain('No valid');
    expect(invoke.callsTo('start_youtube_batch_import_command')).toEqual([]);
  });

  test('startBatch with all-invalid queue sets error state', async () => {
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setQueue([
        { url: 'not a url', title: '', valid: false, error: 'invalid' },
        { url: 'still not', title: '', valid: false, error: 'invalid' },
      ]);
    });

    await act(async () => {
      await result.current.startBatch();
    });

    expect(result.current.status).toBe('error');
    expect(result.current.error).toContain('No valid');
  });

  test('startBatch passes only valid URLs to invoke, in order', async () => {
    invoke.setImpl(async () => 'batch-uuid-1');
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: 'Title A', valid: true, error: null },
        { url: 'not a url', title: '', valid: false, error: 'invalid' },
        { url: 'https://youtu.be/bbb', title: '', valid: true, error: null },
      ]);
    });

    await act(async () => {
      await result.current.startBatch(['Title A', null]);
    });

    const calls = invoke.callsTo('start_youtube_batch_import_command');
    expect(calls.length).toBe(1);
    expect(calls[0].args).toEqual({
      urls: ['https://youtu.be/aaa', 'https://youtu.be/bbb'],
      titles: ['Title A', null],
    });
  });

  test('startBatch passing null titles uses null (not empty array)', async () => {
    invoke.setImpl(async () => 'batch-uuid');
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: '', valid: true, error: null },
      ]);
    });

    await act(async () => {
      await result.current.startBatch();
    });

    const call = invoke.callsTo('start_youtube_batch_import_command')[0];
    expect(call.args).toEqual({
      urls: ['https://youtu.be/aaa'],
      titles: null,
    });
  });

  test('startBatch handles invoke rejection by setting error', async () => {
    invoke.setImpl(async () => {
      throw new Error('No valid YouTube URLs in queue (2 invalid)');
    });
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: '', valid: true, error: null },
      ]);
    });

    await act(async () => {
      await result.current.startBatch();
    });

    expect(result.current.status).toBe('error');
    expect(result.current.error).toContain('No valid YouTube URLs');
  });

  test('startBatch handles non-Error rejection', async () => {
    invoke.setImpl(async () => {
      // eslint-disable-next-line @typescript-eslint/no-throw-literal
      throw 'string error';
    });
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: '', valid: true, error: null },
      ]);
    });

    await act(async () => {
      await result.current.startBatch();
    });

    expect(result.current.status).toBe('error');
    expect(result.current.error).toBe('string error');
  });

  test('progress event updates items list and counters', async () => {
    invoke.setImpl(async () => 'batch-1');
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: '', valid: true, error: null },
        { url: 'https://youtu.be/bbb', title: '', valid: true, error: null },
      ]);
    });
    await act(async () => {
      await result.current.startBatch();
    });

    await act(async () => {
      fireEvent('youtube-batch-progress', {
        id: 'batch-1',
        total: 2,
        completed: 0,
        failed: 0,
        finished: false,
        cancelled: false,
        item: {
          index: 0,
          url: 'https://youtu.be/aaa',
          title: null,
          status: 'downloading',
          progress_percentage: 50,
          meeting_id: null,
          error: null,
        },
      });
    });

    expect(result.current.items.length).toBeGreaterThan(0);
    expect(result.current.items[0].status).toBe('downloading');
    expect(result.current.status).toBe('processing');
  });

  test('cancelBatch invokes cancel command and sets idle', async () => {
    invoke.setImpl(async () => 'batch-1');
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: '', valid: true, error: null },
      ]);
    });
    await act(async () => {
      await result.current.startBatch();
    });

    await act(async () => {
      await result.current.cancelBatch();
    });

    expect(invoke.callsTo('cancel_youtube_batch_import_command').length).toBe(1);
    expect(result.current.status).toBe('idle');
    expect(result.current.isFinished).toBe(true);
  });

  test('reset clears all state', async () => {
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    act(() => {
      result.current.setRawInput('https://youtu.be/aaa');
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: 'T', valid: true, error: null },
      ]);
    });
    act(() => {
      result.current.reset();
    });

    expect(result.current.rawInput).toBe('');
    expect(result.current.queue).toEqual([]);
    expect(result.current.status).toBe('idle');
    expect(result.current.batchId).toBeNull();
    expect(result.current.items).toEqual([]);
    expect(result.current.completed).toBe(0);
    expect(result.current.failed).toBe(0);
    expect(result.current.isFinished).toBe(false);
    expect(result.current.error).toBeNull();
  });

  test('isProcessing reflects status === processing', async () => {
    invoke.setImpl(async () => 'batch-1');
    const { result } = renderHook(() => useYoutubeBatchImport());
    await waitForListeners();

    expect(result.current.isProcessing).toBe(false);

    act(() => {
      result.current.setQueue([
        { url: 'https://youtu.be/aaa', title: '', valid: true, error: null },
      ]);
    });
    await act(async () => {
      await result.current.startBatch();
    });

    // startBatch sets status to 'processing' before await invoke
    // (synchronous up to the await). After invoke resolves, the
    // batchId is set but status remains 'processing' until progress
    // events arrive.
    expect(result.current.isProcessing).toBe(true);
  });
});
