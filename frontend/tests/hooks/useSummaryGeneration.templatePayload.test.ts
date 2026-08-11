import { describe, expect, test } from 'bun:test';
import { buildTemplateInvokePayload } from '../../src/hooks/meeting-details/useSummaryGeneration';
import { GENERATED_TEMPLATE_ID } from '../../src/hooks/meeting-details/useTemplates';
import type { SummaryTemplate } from '../../src/types';

const sampleGeneratedTemplate: SummaryTemplate = {
  name: 'Sprint Retro',
  description: 'Generated for this meeting',
  sections: [
    { title: 'Wins', instruction: 'List wins', format: 'list' },
  ],
};

describe('buildTemplateInvokePayload', () => {
  test('sends templateId "auto" for the auto sentinel', () => {
    expect(buildTemplateInvokePayload('auto', null)).toEqual({ templateId: 'auto' });
  });

  test('sends the concrete templateId for a real template selection', () => {
    expect(buildTemplateInvokePayload('daily_standup', null)).toEqual({ templateId: 'daily_standup' });
  });

  test('sends customTemplateJson (no templateId) when "__generated__" has a cached template', () => {
    const result = buildTemplateInvokePayload(GENERATED_TEMPLATE_ID, sampleGeneratedTemplate);

    expect(result).toEqual({ customTemplateJson: JSON.stringify(sampleGeneratedTemplate) });
    expect(result.templateId).toBeUndefined();
  });

  test('falls back to a fresh auto-select when "__generated__" has no cached template', () => {
    expect(buildTemplateInvokePayload(GENERATED_TEMPLATE_ID, null)).toEqual({ templateId: 'auto' });
  });
});
