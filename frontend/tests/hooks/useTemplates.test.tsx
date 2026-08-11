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
});
