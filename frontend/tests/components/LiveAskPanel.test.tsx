import { describe, expect, mock, test } from 'bun:test';
import { act, fireEvent, waitFor } from '@testing-library/react';
import type { Transcript } from '../../src/types';
import {
  mockTauriInvoke,
  registerAskPanelTestLifecycle,
  renderAskPanel,
  testEnterKeySubmits,
  testRendersEmptyAndDisabled,
} from './askPanelTestUtils';

const invoke = mockTauriInvoke();

// LiveAskPanel reads the in-progress transcript straight from context; the
// real TranscriptProvider would drag in Tauri event listeners and IndexedDB,
// so only the one value the panel consumes is stubbed here.
let mockTranscripts: Partial<Transcript>[] = [];
mock.module('@/contexts/TranscriptContext', () => ({
  useTranscripts: () => ({ transcripts: mockTranscripts }),
}));

const { LiveAskPanel } = await import('../../src/app/_components/LiveAskPanel');

const PLACEHOLDER = /ask about the meeting so far/i;

function renderPanel(transcripts: Partial<Transcript>[]) {
  mockTranscripts = transcripts;
  return renderAskPanel(<LiveAskPanel />, PLACEHOLDER);
}

const TWO_SEGMENTS: Partial<Transcript>[] = [
  { id: '1', text: 'We reviewed the roadmap.' },
  { id: '2', text: 'Then we agreed to ship on Friday.' },
];
const JOINED_TRANSCRIPT = 'We reviewed the roadmap.\nThen we agreed to ship on Friday.';

describe('LiveAskPanel', () => {
  registerAskPanelTestLifecycle(invoke);

  testRendersEmptyAndDisabled(() => renderPanel(TWO_SEGMENTS));

  test('is disabled with a waiting hint when no transcript exists yet', () => {
    const { askButton, questionInput, queryByText } = renderPanel([]);
    fireEvent.change(questionInput(), { target: { value: 'What was said?' } });

    expect(askButton().disabled).toBe(true);
    expect(questionInput().disabled).toBe(true);
    expect(queryByText(/waiting for the first words/i)).not.toBeNull();

    fireEvent.click(askButton());
    expect(invoke.calls).toEqual([]);
  });

  test('submit sends the joined transcript and question to ask_about_live_transcript', () => {
    invoke.setImpl(() => new Promise<string>(() => {}));

    const { askButton, questionInput } = renderPanel(TWO_SEGMENTS);
    fireEvent.change(questionInput(), { target: { value: '  What did we agree?  ' } });
    fireEvent.click(askButton());

    expect(invoke.calls).toEqual([
      {
        cmd: 'ask_about_live_transcript',
        args: { transcript: JOINED_TRANSCRIPT, question: 'What did we agree?' },
      },
    ]);
  });

  test('renders the answer once the command resolves', async () => {
    let resolveInvoke!: (v: string) => void;
    invoke.setImpl(() => new Promise<string>(resolve => { resolveInvoke = resolve; }));

    const { askButton, questionInput, queryByText } = renderPanel(TWO_SEGMENTS);
    fireEvent.change(questionInput(), { target: { value: 'What did we agree?' } });
    fireEvent.click(askButton());

    expect(questionInput().disabled).toBe(true);

    await act(async () => {
      resolveInvoke('You agreed to ship on Friday.');
    });
    await waitFor(() => {
      expect(queryByText('You agreed to ship on Friday.')).not.toBeNull();
    });
    expect(questionInput().disabled).toBe(false);
  });

  test('renders a user-facing error message on reject', async () => {
    invoke.setImpl(() => Promise.reject(new Error('No transcript yet - start speaking and try again.')));

    const { askButton, questionInput, queryByText } = renderPanel(TWO_SEGMENTS);
    fireEvent.change(questionInput(), { target: { value: 'What did we agree?' } });
    await act(async () => {
      fireEvent.click(askButton());
    });

    await waitFor(() => {
      expect(queryByText('No transcript yet - start speaking and try again.')).not.toBeNull();
    });
    expect(queryByText(/at Object\.|\.tsx:\d+/) ?? null).toBeNull();
  });

  testEnterKeySubmits(() => renderPanel(TWO_SEGMENTS), invoke, {
    cmd: 'ask_about_live_transcript',
    args: { transcript: JOINED_TRANSCRIPT, question: 'Question?' },
  });
});
