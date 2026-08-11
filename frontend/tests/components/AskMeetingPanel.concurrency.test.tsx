import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { mockTauriInvoke, renderAskPanel } from './askPanelTestUtils';

// Resolves per-call based on the `meetingId` arg, letting each of two
// concurrently in-flight requests resolve independently and out of order.
let resolvers: Record<string, (v: string) => void> = {};
const invoke = mockTauriInvoke();
invoke.setImpl((_cmd, args: any) => new Promise<string>(resolve => {
  resolvers[args.meetingId] = resolve;
}));

const { AskMeetingPanel } = await import('../../src/components/MeetingDetails/AskMeetingPanel');

function renderPanelFor(meetingId: string) {
  return renderAskPanel(<AskMeetingPanel meetingId={meetingId} />, /ask a question about this meeting/i);
}

describe('AskMeetingPanel concurrent requests to different meetings', () => {
  beforeEach(() => {
    resolvers = {};
    // Not invoke.reset(): that would overwrite the per-meetingId setImpl
    // above with the default no-op, breaking every test in this file.
    invoke.calls.length = 0;
  });

  afterEach(() => {
    cleanup();
    mock.restore();
  });

  test('two AskMeetingPanel instances for two different meetings do not cross-contaminate answers, even when the slower one resolves last', async () => {
    const panelA = renderPanelFor('meeting-A');
    const panelB = renderPanelFor('meeting-B');

    fireEvent.change(panelA.questionInput(), { target: { value: 'What did A decide?' } });
    fireEvent.click(panelA.askButton());

    fireEvent.change(panelB.questionInput(), { target: { value: 'What did B decide?' } });
    fireEvent.click(panelB.askButton());

    expect(invoke.calls).toEqual([
      { cmd: 'ask_about_meeting', args: { meetingId: 'meeting-A', question: 'What did A decide?' } },
      { cmd: 'ask_about_meeting', args: { meetingId: 'meeting-B', question: 'What did B decide?' } },
    ]);

    // Resolve out of order: B (asked second) resolves first.
    await act(async () => {
      resolvers['meeting-B']('Answer for B');
    });
    await waitFor(() => {
      expect(panelB.queryByText('Answer for B')).not.toBeNull();
    });
    expect(panelA.queryByText('Answer for B')).toBeNull();
    expect(panelA.queryByText('Answer for A')).toBeNull();

    await act(async () => {
      resolvers['meeting-A']('Answer for A');
    });
    await waitFor(() => {
      expect(panelA.queryByText('Answer for A')).not.toBeNull();
    });
    expect(panelA.queryByText('Answer for B')).toBeNull();
    expect(panelB.queryByText('Answer for A')).toBeNull();
  });
});
