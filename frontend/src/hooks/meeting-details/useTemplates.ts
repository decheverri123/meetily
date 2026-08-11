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

export function useTemplates() {
  const [fetchedTemplates, setFetchedTemplates] = useState<TemplateOption[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState<string>(AUTO_TEMPLATE_ID);

  const [resolvedTemplateId, setResolvedTemplateId] = useState<string | null>(null);
  const [resolvedTemplateName, setResolvedTemplateName] = useState<string | null>(null);
  const [isGeneratedTemplate, setIsGeneratedTemplate] = useState(false);
  const [generatedTemplate, setGeneratedTemplate] = useState<SummaryTemplate | null>(null);

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

  // Handle template selection
  const handleTemplateSelection = useCallback((templateId: string, templateName: string) => {
    setSelectedTemplate(templateId);
    if (templateId !== GENERATED_TEMPLATE_ID) {
      // Picking anything else (including 'auto') retires the one-use
      // generated-template entry until the next resolution regenerates one.
      setIsGeneratedTemplate(false);
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

    setResolvedTemplateId(data.resolved_template_id ?? null);
    setResolvedTemplateName(data.resolved_template_name);

    if (data.is_generated_template && data.generated_template_json) {
      setGeneratedTemplate(data.generated_template_json);
      setIsGeneratedTemplate(true);
      setSelectedTemplate(GENERATED_TEMPLATE_ID);
    } else {
      setIsGeneratedTemplate(false);
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

    if (isGeneratedTemplate && resolvedTemplateName) {
      synthetic.push({
        id: GENERATED_TEMPLATE_ID,
        name: resolvedTemplateName,
        description: 'Generated for this meeting',
      });
    }

    return [...synthetic, ...fetchedTemplates];
  }, [fetchedTemplates, isGeneratedTemplate, resolvedTemplateName]);

  return {
    availableTemplates,
    selectedTemplate,
    handleTemplateSelection,
    resolvedTemplateId,
    resolvedTemplateName,
    isGeneratedTemplate,
    generatedTemplate,
    applyResolvedTemplate,
  };
}
