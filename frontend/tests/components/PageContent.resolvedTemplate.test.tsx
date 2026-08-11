import { afterEach, describe, expect, mock, test } from 'bun:test';
import { cleanup, render, waitFor } from '@testing-library/react';
import { mockTauriInvoke } from './askPanelTestUtils';

// PageContent pulls in the full meeting-details hook stack (useMeetingData,
// useSummaryGeneration, useCopyOperations, useMeetingOperations) plus heavy
// child panels. Only the template-resolution wiring is under test here, so
// the Sidebar/Config contexts and the panel components are stubbed - none of
// them are exercised by the `summaryData` -> `applyResolvedTemplate` path.
mock.module('@/components/Sidebar/SidebarProvider', () => ({
  useSidebar: () => ({
    serverAddress: '',
    startSummaryPolling: () => {},
    stopSummaryPolling: () => {},
    setCurrentMeeting: () => {},
    setMeetings: () => {},
    meetings: [],
  }),
}));

mock.module('@/contexts/ConfigContext', () => ({
  useConfig: () => ({
    modelConfig: { provider: 'ollama', model: 'gemma3:1b', whisperModel: 'large-v3' },
    setModelConfig: () => {},
    isAutoSummary: true,
  }),
}));

mock.module('@/components/MeetingDetails/TranscriptPanel', () => ({
  TranscriptPanel: () => null,
}));

let lastSummaryPanelProps: any = null;
mock.module('@/components/MeetingDetails/SummaryPanel', () => ({
  SummaryPanel: (props: any) => {
    lastSummaryPanelProps = props;
    return null;
  },
}));

mock.module('@/components/MeetingDetails/AskMeetingPanel', () => ({
  AskMeetingPanel: () => null,
}));

mock.module('@/components/shared/CollapsedPanelRail', () => ({
  CollapsedPanelRail: () => null,
}));

const invoke = mockTauriInvoke();

const { default: PageContent } = await import('../../src/app/meeting-details/page-content');
const { GENERATED_TEMPLATE_ID, AUTO_TEMPLATE_ID } = await import(
  '../../src/hooks/meeting-details/useTemplates'
);

const meeting = { id: 'meeting-1', title: 'Standup', transcripts: [] };

describe('PageContent resolved-template wiring', () => {
  afterEach(() => {
    invoke.reset();
    lastSummaryPanelProps = null;
    cleanup();
  });

  test('reflects an already-generated template from a loaded summaryData prop', async () => {
    invoke.setImpl(async (cmd: string) => (cmd === 'api_list_templates' ? [] : '') as any);

    const summaryData: any = {
      markdown: '# Summary',
      resolved_template_id: null,
      resolved_template_name: 'Sprint Retro',
      is_generated_template: true,
      generated_template_json: {
        name: 'Sprint Retro',
        description: 'Generated for this meeting',
        sections: [{ title: 'Wins', instruction: 'List wins', format: 'list' }],
      },
    };

    render(<PageContent meeting={meeting} summaryData={summaryData} />);

    await waitFor(() => {
      expect(lastSummaryPanelProps?.selectedTemplate).toBe(GENERATED_TEMPLATE_ID);
    });
    expect(
      lastSummaryPanelProps?.availableTemplates.some((t: any) => t.id === GENERATED_TEMPLATE_ID)
    ).toBe(true);
  });

  test('leaves the default "auto" selection for a summary with no resolved-template metadata', async () => {
    invoke.setImpl(async (cmd: string) => (cmd === 'api_list_templates' ? [] : '') as any);

    const summaryData: any = { markdown: '# Summary' };
    render(<PageContent meeting={meeting} summaryData={summaryData} />);

    await waitFor(() => {
      expect(lastSummaryPanelProps).not.toBeNull();
    });
    expect(lastSummaryPanelProps?.selectedTemplate).toBe(AUTO_TEMPLATE_ID);
  });
});
