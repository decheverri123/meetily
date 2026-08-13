import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, renderHook, waitFor } from '@testing-library/react';

// useTemplates.ts imports `invoke` from '@tauri-apps/api/core' at module load
// time, so the mock has to be registered before the hook module is imported
// (see tests/hooks/useLiveActionChips.test.tsx for this pattern).
let invokeImpl: (cmd: string, args?: unknown) => Promise<unknown> = async () => [];
mock.module('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeImpl(cmd, args),
  transformCallback: (callback?: any, once?: boolean) => 0,
  isTauri: () => true,
}));

const { useTemplates, AUTO_TEMPLATE_ID } = await import('../../src/hooks/meeting-details/useTemplates');

describe('useTemplates - YouTube import default_template wiring', () => {
  beforeEach(() => {
    invokeImpl = async () => [];
  });

  afterEach(() => {
    mock.restore();
  });

  test('a stored default_template not present in availableTemplates is NOT applied, leaving the existing selection intact', async () => {
    invokeImpl = async (cmd: string) => {
      if (cmd === 'api_list_templates') {
        // The template list a real user actually has - it does NOT contain
        // "youtube_summary" (e.g. it was renamed/removed by a template pack
        // update, or this build simply predates that built-in template).
        return [
          { id: 'standard_meeting', name: 'Standard Meeting', description: '' },
          { id: 'daily_standup', name: 'Daily Standup', description: '' },
        ];
      }
      if (cmd === 'api_get_meeting_default_template') {
        // What youtube_import.rs's write_import_metadata stores.
        return 'youtube_summary';
      }
      return null;
    };

    const { result } = renderHook(() => useTemplates('meeting-123'));

    await waitFor(() => {
      expect(result.current.availableTemplates.length).toBeGreaterThan(0);
    });

    // The stored default_template ("youtube_summary") isn't in
    // availableTemplates, so it must not be applied - selectedTemplate
    // stays on the hook's existing default instead of pointing at a
    // template id the dropdown can't match.
    expect(result.current.selectedTemplate).toBe(AUTO_TEMPLATE_ID);
    const exists = result.current.availableTemplates.some(t => t.id === result.current.selectedTemplate);
    expect(exists).toBe(true);
  });

  test('meetingId switch (e.g. navigating between meetings in the sidebar) can apply a stale default_template fetch after the user already picked a template for the new meeting', async () => {
    let resolveSecondFetch!: (v: string | null) => void;
    let callCount = 0;

    invokeImpl = async (cmd: string) => {
      if (cmd === 'api_list_templates') {
        return [
          { id: 'standard_meeting', name: 'Standard Meeting', description: '' },
          { id: 'youtube_summary', name: 'YouTube Summary', description: '' },
        ];
      }
      if (cmd === 'api_get_meeting_default_template') {
        callCount += 1;
        if (callCount === 1) {
          return 'youtube_summary';
        }
        // Second meeting's fetch is slow (e.g. cold DB read) and resolves
        // after the user has already manually picked a template.
        return new Promise<string | null>(resolve => { resolveSecondFetch = resolve; });
      }
      return null;
    };

    const { result, rerender } = renderHook(
      ({ meetingId }: { meetingId: string }) => useTemplates(meetingId),
      { initialProps: { meetingId: 'meeting-1' } }
    );

    await waitFor(() => {
      expect(result.current.selectedTemplate).toBe('youtube_summary');
    });

    // Navigate to a different (non-YouTube) meeting.
    rerender({ meetingId: 'meeting-2' });

    // User manually selects a template for meeting-2 before its slow
    // default_template fetch resolves.
    act(() => {
      result.current.handleTemplateSelection('standard_meeting', 'Standard Meeting');
    });
    expect(result.current.selectedTemplate).toBe('standard_meeting');

    // The slow fetch for meeting-2 (kicked off before the user's pick)
    // finally resolves.
    await act(async () => {
      resolveSecondFetch('youtube_summary');
      await new Promise(r => setTimeout(r, 0));
    });

    // Expected: the user's explicit choice sticks. `userSelectedTemplateRef`
    // is a single ref shared across meetings, so once ANY meeting's manual
    // pick sets it, no other meeting's stored default_template is ever
    // applied again for the lifetime of the hook instance - this assertion
    // passes today, but only because the ref is global rather than reset
    // per meetingId, which is a separate correctness smell worth flagging.
    expect(result.current.selectedTemplate).toBe('standard_meeting');
  });
});
