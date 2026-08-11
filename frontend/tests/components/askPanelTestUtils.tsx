import { afterEach, beforeEach, expect, mock, test } from 'bun:test';
import { cleanup, fireEvent, render, within, type RenderResult } from '@testing-library/react';
import type { ReactElement } from 'react';

export type InvokeCall = { cmd: string; args?: unknown };
export type InvokeImpl = (cmd: string, args?: unknown) => Promise<string>;

/**
 * Registers the `@tauri-apps/api/core` mock shared by the Ask-panel test
 * files. Must be called (and its `invoke` export imported) before the
 * component under test, since AskMeetingPanel/GlobalAskPanel import `invoke`
 * at module load time (see tests/hooks/useLiveActionChips.test.tsx for this
 * pattern). Each test swaps behavior via `setImpl` - only the call log and
 * module wiring are shared; per-test resolve/reject/hang behavior stays in
 * the test file.
 */
export function mockTauriInvoke() {
  let impl: InvokeImpl = async () => '';
  const calls: InvokeCall[] = [];
  mock.module('@tauri-apps/api/core', () => ({
    invoke: (cmd: string, args?: unknown) => {
      calls.push({ cmd, args });
      return impl(cmd, args);
    },
  }));
  return {
    calls,
    setImpl: (next: InvokeImpl) => { impl = next; },
    reset: () => {
      calls.length = 0;
      impl = async () => '';
    },
  };
}

/**
 * Renders an Ask-panel element and returns its query surface, scoped to this
 * render's own container (via `within`) rather than the global `screen`, so
 * other test files' un-cleaned-up renders (bun test shares one happy-dom
 * document across files) can't cause ambiguous matches.
 */
export function renderAskPanel(element: ReactElement, placeholder: RegExp) {
  const view = within(render(element).container);
  return {
    askButton: () => view.getByRole('button', { name: /ask/i }) as HTMLButtonElement,
    questionInput: () => view.getByPlaceholderText(placeholder) as HTMLInputElement,
    queryByText: (text: string | RegExp) => view.queryByText(text),
  };
}

/** Like `renderAskPanel`, but also exposes the raw RenderResult (e.g. for `.unmount()`). */
export function renderAskPanelWithResult(element: ReactElement, placeholder: RegExp) {
  const result: RenderResult = render(element);
  const view = within(result.container);
  return {
    result,
    askButton: () => view.getByRole('button', { name: /ask/i }) as HTMLButtonElement,
    questionInput: () => view.getByPlaceholderText(placeholder) as HTMLInputElement,
    queryByText: (text: string | RegExp) => view.queryByText(text),
  };
}

/**
 * Registers the `invoke.reset()` / `cleanup()` + `mock.restore()` pair every
 * Ask-panel test file needs between tests. Shared because all of them need
 * the exact same pair - independently written across
 * AskMeetingPanel.test.tsx, GlobalAskPanel.test.tsx, and
 * AskMeetingPanel.breaker.test.tsx, they'd otherwise repeat it verbatim.
 */
export function registerAskPanelTestLifecycle(invoke: ReturnType<typeof mockTauriInvoke>) {
  beforeEach(() => {
    invoke.reset();
  });
  afterEach(() => {
    cleanup();
    mock.restore();
  });
}

/** Shared "empty input renders with a disabled submit button" case for an Ask panel. */
export function testRendersEmptyAndDisabled(renderPanel: () => ReturnType<typeof renderAskPanel>) {
  test('renders with empty input and a disabled submit button', () => {
    const { askButton, questionInput } = renderPanel();
    expect(questionInput().value).toBe('');
    expect(askButton().disabled).toBe(true);
  });
}

/** Shared "Enter key submits the question" case for an Ask panel. */
export function testEnterKeySubmits(
  renderPanel: () => ReturnType<typeof renderAskPanel>,
  invoke: ReturnType<typeof mockTauriInvoke>,
  expectedCall: InvokeCall
) {
  test('Enter key submits the question', () => {
    // Never-resolving promise: this test only cares that Enter triggers the
    // invoke() call, not its resolution - avoids a dangling state update
    // firing after the test (and its render) has already torn down.
    invoke.setImpl(() => new Promise<string>(() => {}));
    const { questionInput } = renderPanel();
    fireEvent.change(questionInput(), { target: { value: 'Question?' } });
    fireEvent.keyDown(questionInput(), { key: 'Enter', code: 'Enter' });

    expect(invoke.calls).toEqual([expectedCall]);
  });
}
