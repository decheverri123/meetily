import { afterEach, describe, expect, test } from 'bun:test';
import { cleanup, renderHook, act, waitFor } from '@testing-library/react';
import { mockTauriInvoke } from '../components/askPanelTestUtils';

const invoke = mockTauriInvoke();

const { AUTO_TEMPLATE_ID, GENERATED_TEMPLATE_ID, useTemplates } = await import(
  '../../src/hooks/meeting-details/useTemplates'
);

async function renderTemplates() {
  invoke.setImpl(async () => [] as any);
  const rendered = renderHook(() => useTemplates());
  await waitFor(() => expect(rendered.result.current.availableTemplates.length).toBeGreaterThan(0));
  return rendered;
}

describe('useTemplates', () => {
  afterEach(() => {
    invoke.reset();
    cleanup();
  });

  test('defaults selectedTemplate to "auto"', async () => {
    const { result } = await renderTemplates();

    expect(result.current.selectedTemplate).toBe(AUTO_TEMPLATE_ID);
  });

  test('exposes only the Auto entry before any generation has happened', async () => {
    const { result } = await renderTemplates();

    expect(result.current.availableTemplates).toEqual([
      { id: AUTO_TEMPLATE_ID, name: 'Auto (recommended)', description: 'Automatically pick the best template for this meeting' },
    ]);
  });

  test('adds a "__generated__" entry and switches selection when a generated template resolves', async () => {
    const { result } = await renderTemplates();

    act(() => {
      result.current.applyResolvedTemplate({
        resolved_template_id: null,
        resolved_template_name: 'Sprint Retro',
        is_generated_template: true,
        generated_template_json: {
          name: 'Sprint Retro',
          description: 'Generated for this meeting',
          sections: [{ title: 'Wins', instruction: 'List wins', format: 'list' }],
        },
      });
    });

    expect(result.current.selectedTemplate).toBe(GENERATED_TEMPLATE_ID);
    expect(result.current.isGeneratedTemplate).toBe(true);
    expect(result.current.availableTemplates.map(t => t.id)).toEqual([AUTO_TEMPLATE_ID, GENERATED_TEMPLATE_ID]);
    expect(result.current.availableTemplates[1].name).toBe('Sprint Retro');
  });

  test('drops the "__generated__" entry once the user picks a different template', async () => {
    const { result } = await renderTemplates();

    act(() => {
      result.current.applyResolvedTemplate({
        resolved_template_name: 'Sprint Retro',
        is_generated_template: true,
        generated_template_json: { name: 'Sprint Retro', description: 'x', sections: [{ title: 'Wins', instruction: 'x', format: 'list' }] },
      });
    });
    expect(result.current.availableTemplates.some(t => t.id === GENERATED_TEMPLATE_ID)).toBe(true);

    act(() => {
      result.current.handleTemplateSelection('daily_standup', 'Daily Standup');
    });

    expect(result.current.selectedTemplate).toBe('daily_standup');
    expect(result.current.availableTemplates.some(t => t.id === GENERATED_TEMPLATE_ID)).toBe(false);
  });

  test('applyResolvedTemplate ignores legacy responses without resolved_template_name', async () => {
    const { result } = await renderTemplates();

    act(() => {
      result.current.applyResolvedTemplate({ markdown: 'some legacy markdown' } as any);
    });

    expect(result.current.selectedTemplate).toBe(AUTO_TEMPLATE_ID);
    expect(result.current.isGeneratedTemplate).toBe(false);
  });

  test('applyResolvedTemplate selects the matched existing template, not just generated ones', async () => {
    const { result } = await renderTemplates();

    act(() => {
      result.current.applyResolvedTemplate({
        resolved_template_id: 'daily_standup',
        resolved_template_name: 'Daily Standup',
        is_generated_template: false,
        generated_template_json: null,
      } as any);
    });

    expect(result.current.selectedTemplate).toBe('daily_standup');
    expect(result.current.isGeneratedTemplate).toBe(false);
    expect(result.current.generatedTemplate).toBeNull();
  });

  test('resets selection and generated template when the meeting id changes', async () => {
    invoke.setImpl(async () => [] as any);
    const { result, rerender } = renderHook(
      ({ meetingId }) => useTemplates(meetingId),
      { initialProps: { meetingId: 'meeting-a' } }
    );
    await waitFor(() => expect(result.current.availableTemplates.length).toBeGreaterThan(0));

    act(() => {
      result.current.applyResolvedTemplate({
        resolved_template_name: 'Sprint Retro',
        is_generated_template: true,
        generated_template_json: {
          name: 'Sprint Retro',
          description: 'x',
          sections: [{ title: 'Wins', instruction: 'x', format: 'list' }],
        },
      } as any);
    });
    expect(result.current.selectedTemplate).toBe(GENERATED_TEMPLATE_ID);

    rerender({ meetingId: 'meeting-b' });

    expect(result.current.selectedTemplate).toBe(AUTO_TEMPLATE_ID);
    expect(result.current.generatedTemplate).toBeNull();
    expect(result.current.availableTemplates.some(t => t.id === GENERATED_TEMPLATE_ID)).toBe(false);
  });

  // `applyResolvedTemplate` is a stable (empty-deps) callback that is handed
  // to `startSummaryPolling`'s onUpdate closure while the user is on meeting
  // A. `PageContent` never remounts across meeting navigation (per the
  // meeting-switch reset effect's own comment above), so if meeting A's
  // summary-generation poll finishes AFTER the user has already navigated to
  // meeting B, the stale closure still calls into the SAME `useTemplates`
  // instance now representing meeting B. Nothing in `applyResolvedTemplate`
  // checks that the resolved data actually belongs to the meeting the hook
  // currently represents.
  test('a stale poll resolving after a meeting switch corrupts the new meeting template state', async () => {
    invoke.setImpl(async () => [] as any);
    const { result, rerender } = renderHook(
      ({ meetingId }) => useTemplates(meetingId),
      { initialProps: { meetingId: 'meeting-a' } }
    );
    await waitFor(() => expect(result.current.availableTemplates.length).toBeGreaterThan(0));

    // User navigates away from meeting-a before its in-flight summary
    // generation poll resolves.
    rerender({ meetingId: 'meeting-b' });
    expect(result.current.selectedTemplate).toBe(AUTO_TEMPLATE_ID);

    // meeting-a's poll now completes late and invokes the stale
    // onTemplateResolved/applyResolvedTemplate callback captured before the
    // switch (simulated directly here since applyResolvedTemplate's identity
    // does not change across the meetingId switch).
    act(() => {
      result.current.applyResolvedTemplate({
        resolved_template_id: null,
        resolved_template_name: 'Meeting A Retro',
        is_generated_template: true,
        generated_template_json: {
          name: 'Meeting A Retro',
          description: 'Generated for meeting A',
          sections: [{ title: 'Wins', instruction: 'List wins', format: 'list' }],
        },
      } as any);
    });

    // Expected: meeting-b's template state must be unaffected by meeting-a's
    // stale resolution. Actual: it gets overwritten with meeting-a's data.
    expect(result.current.selectedTemplate).toBe(AUTO_TEMPLATE_ID);
    expect(result.current.generatedTemplate).toBeNull();
  });
});
