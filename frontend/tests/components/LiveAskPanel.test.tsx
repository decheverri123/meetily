import { describe, expect, mock, test } from 'bun:test';
import { act, fireEvent, render, waitFor, within } from '@testing-library/react';
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

// Pinned action-chips row - not under test here, so a static, always-idle
// stand-in keeps it out of the way (isRecording: false, hasActivity: false
// hides the row entirely, matching LiveAskPanel's own gating).
const NO_LIVE_ACTION_CHIPS = {
  chips: {
    recap: { result: '', isLoading: false, error: null, isRetryable: false, hasGenerated: false },
    questions: { result: '', isLoading: false, error: null, isRetryable: false, hasGenerated: false },
  },
  generate: () => {},
  hasActivity: false,
  isRecording: false,
};
const BASE_PROPS = {
  liveActionChips: NO_LIVE_ACTION_CHIPS,
  liveActionChipOverride: null,
  onLiveActionChipOverrideChange: () => {},
  isRecording: false,
};

function renderPanel(transcripts: Partial<Transcript>[]) {
  mockTranscripts = transcripts;
  return renderAskPanel(<LiveAskPanel {...BASE_PROPS} />, PLACEHOLDER);
}

const TWO_SEGMENTS: Partial<Transcript>[] = [
  { id: '1', text: 'We reviewed the roadmap.', audio_start_time: 12 },
  { id: '2', text: 'Then we agreed to ship on Friday.', audio_start_time: 84 },
];
// Each line carries its [MM:SS] stamp so the model can cite it back; the
// citation chips are matched against these same stamps.
const JOINED_TRANSCRIPT =
  '[00:12] We reviewed the roadmap.\n[01:24] Then we agreed to ship on Friday.';

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

  test('keeps earlier exchanges in the thread and empties the input for the next one', async () => {
    invoke.setImpl(async (_cmd, args) =>
      (args as { question: string }).question === 'First?' ? 'First answer.' : 'Second answer.'
    );

    const { askButton, questionInput, queryByText } = renderPanel(TWO_SEGMENTS);

    for (const question of ['First?', 'Second?']) {
      fireEvent.change(questionInput(), { target: { value: question } });
      await act(async () => {
        fireEvent.click(askButton());
      });
    }

    await waitFor(() => {
      expect(queryByText('Second answer.')).not.toBeNull();
    });
    expect(queryByText('First answer.')).not.toBeNull();
    expect(queryByText('First?')).not.toBeNull();
    expect(questionInput().value).toBe('');
  });

  test('renders [MM:SS] citations as chips and reports the segments they cite', async () => {
    invoke.setImpl(async () => 'You agreed to ship [01:24], after the roadmap review [00:12].');
    const cited: string[][] = [];

    mockTranscripts = TWO_SEGMENTS;
    const view = within(
      render(<LiveAskPanel {...BASE_PROPS} onCitedSegmentsChange={ids => cited.push(ids)} />).container
    );

    fireEvent.change(view.getByPlaceholderText(PLACEHOLDER), { target: { value: 'What did we agree?' } });
    await act(async () => {
      fireEvent.click(view.getByRole('button', { name: 'Ask' }));
    });

    await waitFor(() => {
      expect(view.queryByText('01:24')).not.toBeNull();
    });
    expect(view.queryByText('00:12')).not.toBeNull();
    // Reported in transcript order, not citation order.
    expect(cited[cited.length - 1]).toEqual(['1', '2']);
  });

  test('a citation with no matching segment stays inert rather than pointing at the wrong line', async () => {
    invoke.setImpl(async () => 'Mentioned earlier [00:02].');

    const focused: string[] = [];
    mockTranscripts = TWO_SEGMENTS;
    const view = within(
      render(<LiveAskPanel {...BASE_PROPS} onFocusSegment={id => focused.push(id)} />).container
    );

    fireEvent.change(view.getByPlaceholderText(PLACEHOLDER), { target: { value: 'When?' } });
    await act(async () => {
      fireEvent.click(view.getByRole('button', { name: 'Ask' }));
    });

    await waitFor(() => {
      expect(view.queryByText('00:02')).not.toBeNull();
    });
    const chip = view.getByText('00:02') as HTMLButtonElement;
    expect(chip.disabled).toBe(true);
    fireEvent.click(chip);
    expect(focused).toEqual([]);
  });
});
