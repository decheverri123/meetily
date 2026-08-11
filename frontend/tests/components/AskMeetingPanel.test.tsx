import { describe, expect, test } from 'bun:test';
import { act, fireEvent, waitFor } from '@testing-library/react';
import {
  mockTauriInvoke,
  registerAskPanelTestLifecycle,
  renderAskPanel,
  testEnterKeySubmits,
  testRendersEmptyAndDisabled,
} from './askPanelTestUtils';

const invoke = mockTauriInvoke();

const { AskMeetingPanel } = await import('../../src/components/MeetingDetails/AskMeetingPanel');

function renderPanel() {
  return renderAskPanel(<AskMeetingPanel meetingId="meeting-1" />, /ask a question about this meeting/i);
}

describe('AskMeetingPanel', () => {
  registerAskPanelTestLifecycle(invoke);

  testRendersEmptyAndDisabled(renderPanel);

  test('whitespace-only question keeps submit disabled', () => {
    const { askButton, questionInput } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: '   ' } });
    expect(askButton().disabled).toBe(true);
  });

  test('submit calls ask_about_meeting with the meeting id and trimmed question', async () => {
    let resolveInvoke!: (v: string) => void;
    invoke.setImpl(() => new Promise<string>(resolve => { resolveInvoke = resolve; }));

    const { askButton, questionInput, queryByText } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: 'What did we decide?' } });
    fireEvent.click(askButton());

    expect(invoke.calls).toEqual([
      { cmd: 'ask_about_meeting', args: { meetingId: 'meeting-1', question: 'What did we decide?' } },
    ]);

    // Loading state: input and button disabled while the promise is pending.
    expect(askButton().disabled).toBe(true);
    expect(questionInput().disabled).toBe(true);

    await act(async () => {
      resolveInvoke('They decided to ship on Friday.');
    });
    await waitFor(() => {
      expect(queryByText('They decided to ship on Friday.')).not.toBeNull();
    });
    expect(questionInput().disabled).toBe(false);
  });

  test('shows a user-facing error message on reject, not a raw stack trace', async () => {
    invoke.setImpl(() => Promise.reject(new Error('Model unavailable')));

    const { askButton, questionInput, queryByText } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: 'Question?' } });
    await act(async () => {
      fireEvent.click(askButton());
    });

    await waitFor(() => {
      expect(queryByText('Model unavailable')).not.toBeNull();
    });
    expect(queryByText(/at Object\.|\.tsx:\d+/) ?? null).toBeNull();
  });

  testEnterKeySubmits(renderPanel, invoke, {
    cmd: 'ask_about_meeting',
    args: { meetingId: 'meeting-1', question: 'Question?' },
  });
});
