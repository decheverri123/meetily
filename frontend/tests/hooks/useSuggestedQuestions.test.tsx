import { afterEach, describe, expect, mock, test } from 'bun:test';
import { act, cleanup, render, waitFor, within } from '@testing-library/react';
import { mockTauriInvoke } from '../components/askPanelTestUtils';

const invoke = mockTauriInvoke();

const { parseSuggestedQuestions, useSuggestedQuestions } = await import(
  '../../src/hooks/useSuggestedQuestions'
);

// Each test gets its own scope: generated suggestions are cached per scope for
// the life of the module, so a shared one would leak between tests.
function Probe({ scope, enabled = true }: { scope: string; enabled?: boolean }) {
  const suggestions = useSuggestedQuestions({
    command: 'suggest_meeting_questions',
    args: { meetingId: scope },
    scope,
    enabled,
  });
  return <ul>{suggestions.map(s => <li key={s}>{s}</li>)}</ul>;
}

describe('parseSuggestedQuestions', () => {
  test('takes bare one-per-line questions as written', () => {
    expect(parseSuggestedQuestions('Who owns pricing?\nWhat slipped?')).toEqual([
      'Who owns pricing?',
      'What slipped?',
    ]);
  });

  test('strips the list markers, numbering, and quotes models add anyway', () => {
    const reply = '- Who owns pricing?\n2. What slipped?\n• "Why the delay?"';

    expect(parseSuggestedQuestions(reply)).toEqual([
      'Who owns pricing?',
      'What slipped?',
      'Why the delay?',
    ]);
  });

  test('drops preamble and closing lines, which are never questions', () => {
    const reply = 'Here are three questions:\nWho owns pricing?\nLet me know if you need more.';

    expect(parseSuggestedQuestions(reply)).toEqual(['Who owns pricing?']);
  });

  test('drops questions too long to read as a chip', () => {
    const long = `Could you walk me through ${'every last detail '.repeat(6)}please?`;

    expect(parseSuggestedQuestions(`Short one?\n${long}`)).toEqual(['Short one?']);
  });

  test('de-duplicates case-insensitively and caps at three', () => {
    const reply = 'A?\na?\nB?\nC?\nD?';

    expect(parseSuggestedQuestions(reply)).toEqual(['A?', 'B?', 'C?']);
  });

  test('an unusable reply yields nothing, leaving the caller its fallback', () => {
    expect(parseSuggestedQuestions('I could not find anything to ask about.')).toEqual([]);
    expect(parseSuggestedQuestions('')).toEqual([]);
  });
});

describe('useSuggestedQuestions', () => {
  afterEach(() => {
    invoke.reset();
    cleanup();
    mock.restore();
  });

  test('shows nothing until meeting-specific questions arrive', async () => {
    const scope = 'replaces';
    invoke.setImpl(async () => 'Who owns compliance?\nWhat blocked the rollout?');

    const view = within(render(<Probe scope={scope} />).container);
    expect(view.queryAllByRole('listitem')).toHaveLength(0);
    await waitFor(() => {
      expect(view.queryByText('Who owns compliance?')).not.toBeNull();
    });
  });

  test('stays empty when generation fails', async () => {
    const scope = 'fails';
    invoke.setImpl(() => Promise.reject(new Error('No model configured')));

    const view = within(render(<Probe scope={scope} />).container);
    await act(async () => {});

    expect(view.queryAllByRole('listitem')).toHaveLength(0);
  });

  test('stays empty when the reply has no usable question', async () => {
    const scope = 'unusable';
    invoke.setImpl(async () => 'Nothing has been discussed yet.');

    const view = within(render(<Probe scope={scope} />).container);
    await act(async () => {});

    expect(view.queryAllByRole('listitem')).toHaveLength(0);
  });

  test('does not call the backend while disabled, and stays empty', async () => {
    const scope = 'disabled';
    const view = within(render(<Probe scope={scope} enabled={false} />).container);
    await act(async () => {});

    expect(invoke.calls).toEqual([]);
    expect(view.queryAllByRole('listitem')).toHaveLength(0);
  });
});
