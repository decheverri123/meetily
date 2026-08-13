import { describe, expect, test } from 'bun:test';
import { act, fireEvent, waitFor } from '@testing-library/react';
import {
  mockTauriInvoke,
  registerAskPanelTestLifecycle,
  renderAskPanel,
  renderAskPanelWithResult,
} from './askPanelTestUtils';

const invoke = mockTauriInvoke();

const { AskMeetingPanel } = await import('../../src/components/MeetingDetails/AskMeetingPanel');

function renderPanel() {
  return renderAskPanel(<AskMeetingPanel meetingId="meeting-1" />, /ask a question about this meeting/i);
}

describe('AskMeetingPanel adversarial', () => {
  registerAskPanelTestLifecycle(invoke);

  test('rapid double-click before React flushes isLoading only invokes once', () => {
    invoke.setImpl(() => new Promise<string>(() => {})); // never resolves
    const { askButton, questionInput } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: 'Question?' } });

    // Two synchronous clicks in the same tick, mirroring a real rapid
    // double-click before React has a chance to commit isLoading=true and
    // disable the button.
    const btn = askButton();
    fireEvent.click(btn);
    fireEvent.click(btn);

    expect(invoke.callsTo('ask_about_meeting').length).toBe(1);
  });

  test('non-Error rejection value (plain object) should not surface "[object Object]" to the user', async () => {
    // Tauri commands normally reject with a plain string (the Rust Err(String)),
    // but a bug/panic-adjacent path (or a future refactor) could reject with a
    // structured object instead. useAskAI's catch does
    // `err instanceof Error ? err.message : String(err)`, which stringifies
    // any non-Error object via Object.prototype.toString -> "[object Object]".
    invoke.setImpl(() => Promise.reject({ code: 'MODEL_UNAVAILABLE', message: 'Model unavailable' }));

    const { askButton, questionInput, queryByText } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: 'Question?' } });
    await act(async () => {
      fireEvent.click(askButton());
    });

    await waitFor(() => {
      expect(queryByText(/\[object Object\]/)).toBeNull();
    });
  });

  test('unmounting while a request is in flight does not throw / warn about setState on unmounted component', async () => {
    let resolveInvoke!: (v: string) => void;
    invoke.setImpl(() => new Promise<string>(resolve => { resolveInvoke = resolve; }));

    const originalError = console.error;
    const errors: unknown[] = [];
    console.error = (...args: unknown[]) => { errors.push(args); };

    const { result, askButton, questionInput } = renderAskPanelWithResult(
      <AskMeetingPanel meetingId="meeting-1" />,
      /ask a question about this meeting/i
    );
    fireEvent.change(questionInput(), { target: { value: 'Question?' } });
    fireEvent.click(askButton());

    result.unmount();

    await act(async () => {
      resolveInvoke('Late answer after unmount');
      await Promise.resolve();
      await Promise.resolve();
    });

    console.error = originalError;

    const stateUpdateWarning = errors.find((argSet: any) =>
      String(argSet[0]).includes('unmounted component')
    );
    expect(stateUpdateWarning).toBeUndefined();
  });
});
