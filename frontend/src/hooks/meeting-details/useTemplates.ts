import { useState, useEffect, useCallback, useMemo } from 'react';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { SummaryDataResponse, SummaryTemplate } from '@/types';

export const AUTO_TEMPLATE_ID = 'auto';
export const GENERATED_TEMPLATE_ID = '__generated__';

export interface TemplateOption {
  id: string;
  name: string;
  description: string;
}

export function useTemplates(meetingId?: string) {
  const [fetchedTemplates, setFetchedTemplates] = useState<TemplateOption[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState<string>(AUTO_TEMPLATE_ID);
  const [generatedTemplate, setGeneratedTemplate] = useState<SummaryTemplate | null>(null);
  const isGeneratedTemplate = generatedTemplate !== null;

  // Fetch available templates on mount
  useEffect(() => {
    const fetchTemplates = async () => {
      try {
        const templates = await invokeTauri('api_list_templates') as TemplateOption[];
        console.log('Available templates:', templates);
        setFetchedTemplates(templates);
      } catch (error) {
        console.error('Failed to fetch templates:', error);
      }
    };
    fetchTemplates();
  }, []);

  // PageContent persists across meeting navigation (meetingId changes via a
  // search param, no remount), so template selection must be reset explicitly
  // on meeting switch rather than only updated when new resolution data
  // arrives — otherwise a generated template from meeting A leaks into
  // meeting B's dropdown/regenerate state. Mirrors the resync-on-prop-change
  // pattern useMeetingData.ts uses for its own per-meeting state.
  useEffect(() => {
    setSelectedTemplate(AUTO_TEMPLATE_ID);
    setGeneratedTemplate(null);
  }, [meetingId]);

  // Handle template selection
  const handleTemplateSelection = useCallback((templateId: string, templateName: string) => {
    setSelectedTemplate(templateId);
    if (templateId !== GENERATED_TEMPLATE_ID) {
      // Picking anything else (including 'auto') retires the one-use
      // generated-template entry until the next resolution regenerates one.
      setGeneratedTemplate(null);
    }
    toast.success('Template selected', {
      description: `Using "${templateName}" template for summary generation`,
    });
    Analytics.trackFeatureUsed('template_selected');
  }, []);

  // Feeds the auto-template-selection metadata from a polled `api_get_summary`
  // `data` blob (see SummaryDataResponse) back into template state, so the
  // dropdown reflects what was actually used and a generated one-off template
  // can be replayed on Regenerate without another LLM call.
  const applyResolvedTemplate = useCallback((data: SummaryDataResponse | null | undefined) => {
    if (!data || data.resolved_template_name === undefined) {
      return;
    }

    if (data.is_generated_template && data.generated_template_json) {
      setGeneratedTemplate(data.generated_template_json);
      setSelectedTemplate(GENERATED_TEMPLATE_ID);
    } else {
      setGeneratedTemplate(null);
      if (data.resolved_template_id) {
        // Auto-select matched an existing template rather than generating
        // one — reflect that choice in the dropdown too, not just the
        // generated-template case.
        setSelectedTemplate(data.resolved_template_id);
      }
    }
  }, []);

  const availableTemplates = useMemo<TemplateOption[]>(() => {
    const synthetic: TemplateOption[] = [
      {
        id: AUTO_TEMPLATE_ID,
        name: 'Auto (recommended)',
        description: 'Automatically pick the best template for this meeting',
      },
    ];

    if (generatedTemplate) {
      synthetic.push({
        id: GENERATED_TEMPLATE_ID,
        name: generatedTemplate.name,
        description: 'Generated for this meeting',
      });
    }

    return [...synthetic, ...fetchedTemplates];
  }, [fetchedTemplates, generatedTemplate]);

  return {
    availableTemplates,
    selectedTemplate,
    handleTemplateSelection,
    isGeneratedTemplate,
    generatedTemplate,
    applyResolvedTemplate,
  };
}
