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

const { GlobalAskPanel } = await import('../../src/components/Sidebar/GlobalAskPanel');

function renderPanel() {
  return renderAskPanel(<GlobalAskPanel />, /ask across all meetings/i);
}

describe('GlobalAskPanel', () => {
  registerAskPanelTestLifecycle(invoke);

  testRendersEmptyAndDisabled(renderPanel);

  test('submit calls ask_across_meetings with only the trimmed question (no meetingId)', async () => {
    let resolveInvoke!: (v: string) => void;
    invoke.setImpl(() => new Promise<string>(resolve => { resolveInvoke = resolve; }));

    const { askButton, questionInput, queryByText } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: '  Which meetings mentioned pricing?  ' } });
    fireEvent.click(askButton());

    expect(invoke.calls).toEqual([
      { cmd: 'ask_across_meetings', args: { question: 'Which meetings mentioned pricing?' } },
    ]);

    // Loading state while the promise is pending.
    expect(askButton().disabled).toBe(true);
    expect(questionInput().disabled).toBe(true);

    await act(async () => {
      resolveInvoke('Meetings A and C mentioned pricing.');
    });
    await waitFor(() => {
      expect(queryByText('Meetings A and C mentioned pricing.')).not.toBeNull();
    });
  });

  test('shows a user-facing error message on reject', async () => {
    invoke.setImpl(() => Promise.reject(new Error('No meetings indexed yet')));

    const { askButton, questionInput, queryByText } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: 'Question?' } });
    await act(async () => {
      fireEvent.click(askButton());
    });

    await waitFor(() => {
      expect(queryByText('No meetings indexed yet')).not.toBeNull();
    });
  });

  testEnterKeySubmits(renderPanel, invoke, {
    cmd: 'ask_across_meetings',
    args: { question: 'Question?' },
  });
});
