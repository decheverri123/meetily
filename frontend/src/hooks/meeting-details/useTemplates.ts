import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';

export function useTemplates(meetingId?: string) {
  const [availableTemplates, setAvailableTemplates] = useState<Array<{
    id: string;
    name: string;
    description: string;
  }>>([]);
  const [selectedTemplate, setSelectedTemplate] = useState<string>('standard_meeting');

  // Once the user manually picks a template, stop letting the meeting's
  // stored default (see effect below) override that choice.
  const userSelectedTemplateRef = useRef(false);

  // Mirrors `availableTemplates` synchronously (updated the instant the fetch
  // resolves, not on the next render), so the default-template effect below
  // can check membership against the real list even if it resolves before
  // React has re-rendered with the fetched templates.
  const availableTemplatesRef = useRef(availableTemplates);

  // Fetch available templates on mount
  useEffect(() => {
    const fetchTemplates = async () => {
      try {
        const templates = await invokeTauri('api_list_templates') as Array<{
          id: string;
          name: string;
          description: string;
        }>;
        console.log('Available templates:', templates);
        availableTemplatesRef.current = templates;
        setAvailableTemplates(templates);
      } catch (error) {
        console.error('Failed to fetch templates:', error);
      }
    };
    fetchTemplates();
  }, []);

  // Apply the meeting's stored default template (e.g. "youtube_summary" for
  // meetings created via YouTube import), read from metadata.json, unless
  // the user has already picked a template manually.
  useEffect(() => {
    if (!meetingId || userSelectedTemplateRef.current) return;

    let cancelled = false;
    const applyStoredDefaultTemplate = async () => {
      try {
        const defaultTemplate = await invokeTauri<string | null>('api_get_meeting_default_template', {
          meetingId,
        });
        const templateExists = availableTemplatesRef.current.some((t) => t.id === defaultTemplate);
        if (!cancelled && defaultTemplate && templateExists && !userSelectedTemplateRef.current) {
          setSelectedTemplate(defaultTemplate);
        }
      } catch (error) {
        console.error('Failed to fetch meeting default template:', error);
      }
    };
    applyStoredDefaultTemplate();

    return () => { cancelled = true; };
  }, [meetingId]);

  // Handle template selection
  const handleTemplateSelection = useCallback((templateId: string, templateName: string) => {
    userSelectedTemplateRef.current = true;
    setSelectedTemplate(templateId);
    toast.success('Template selected', {
      description: `Using "${templateName}" template for summary generation`,
    });
    Analytics.trackFeatureUsed('template_selected');
  }, []);

  return {
    availableTemplates,
    selectedTemplate,
    handleTemplateSelection,
  };
}
