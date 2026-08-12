'use client';

import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, ChevronsUpDown, RefreshCw, SlidersHorizontal } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import { useConfig } from '@/contexts/ConfigContext';
import { cn } from '@/lib/utils';
import { providerLabel } from '@/lib/providerLabels';
import type { LiveActionChipModelOverride } from '@/hooks/useLiveActionChips';

/**
 * Providers offered here, i.e. those with a real model-listing Tauri command.
 * `custom-openai` is deliberately excluded - it has no listing endpoint (the
 * model name is free text in Settings), which doesn't fit this compact picker.
 */
type OverrideProvider = 'builtin-ai' | 'claude' | 'groq' | 'ollama' | 'openai' | 'openrouter';

/** Providers that require a saved key, checked against `useConfig().providerApiKeys`. */
type KeyedProvider = 'claude' | 'groq' | 'openai' | 'openrouter';

/**
 * Usable with no saved API key - i.e. generation never leaves the device.
 * Exported so `LiveProviderIndicator` can classify the effective provider as
 * local vs. cloud using this exact same list, rather than re-deriving it.
 */
export const ALWAYS_AVAILABLE_PROVIDERS: OverrideProvider[] = ['builtin-ai', 'ollama'];
const KEYED_PROVIDERS: KeyedProvider[] = ['claude', 'groq', 'openai', 'openrouter'];

// `LiveActionChipModelOverride.provider` is a plain `string` (it mirrors the
// Rust-side invoke arg, which accepts any provider id), so narrowing it back
// to `OverrideProvider` needs a runtime check rather than an `as` cast.
const OVERRIDE_PROVIDERS: OverrideProvider[] = [
  'builtin-ai',
  'claude',
  'groq',
  'ollama',
  'openai',
  'openrouter',
];

function isOverrideProvider(value: string): value is OverrideProvider {
  return (OVERRIDE_PROVIDERS as string[]).includes(value);
}

function isKeyedProvider(provider: OverrideProvider): provider is KeyedProvider {
  return (KEYED_PROVIDERS as OverrideProvider[]).includes(provider);
}

/** "<Provider> · <model>" footer label shared by the live and saved-meeting ask panels. */
export function modelConfigLabel(config: { provider: string; model: string }): string {
  return `${providerLabel(config.provider)} · ${config.model || 'default'}`;
}

async function fetchModelNames(provider: OverrideProvider, apiKey: string | null): Promise<string[]> {
  switch (provider) {
    case 'ollama': {
      const models = await invoke<{ name: string }[]>('get_ollama_models', { endpoint: null });
      return models.map(m => m.name);
    }
    case 'builtin-ai': {
      const models = await invoke<{ name: string }[]>('builtin_ai_list_models');
      return models.map(m => m.name);
    }
    case 'claude': {
      const models = await invoke<{ id: string }[]>('get_anthropic_models', { apiKey });
      return models.map(m => m.id);
    }
    case 'groq': {
      const models = await invoke<{ id: string }[]>('get_groq_models', { apiKey });
      return models.map(m => m.id);
    }
    case 'openai': {
      const models = await invoke<{ id: string }[]>('get_openai_models', { apiKey });
      return models.map(m => m.id);
    }
    case 'openrouter': {
      const models = await invoke<{ id: string }[]>('get_openrouter_models');
      return models.map(m => m.id);
    }
  }
}

interface LiveActionChipModelPickerProps {
  /** Current session-only override, or null to use the Settings default. */
  override: LiveActionChipModelOverride | null;
  onOverrideChange: (override: LiveActionChipModelOverride | null) => void;
}

/**
 * Compact icon-button + popover for overriding which provider/model powers
 * the next live action chip generation, without opening full Settings.
 * Mirrors the provider Select + model combobox pattern from
 * ModelSettingsModal.tsx, trimmed to this control's smaller footprint.
 *
 * The override is owned by the caller (page-level state) and is session-only
 * - not persisted to the DB or localStorage. Only providers with a usable
 * saved key (plus always-available Ollama/Built-in AI) are offered.
 */
export function LiveActionChipModelPicker({ override, onOverrideChange }: LiveActionChipModelPickerProps) {
  const { providerApiKeys, modelConfig } = useConfig();

  const [open, setOpen] = useState(false);
  const [modelPopoverOpen, setModelPopoverOpen] = useState(false);
  // Draft provider selection - becomes the committed override once a model is picked.
  const [draftProvider, setDraftProvider] = useState<OverrideProvider | null>(
    override && isOverrideProvider(override.provider) ? override.provider : null
  );
  // `undefined` = never attempted, `null` = last attempt failed (retry on
  // next selection), `string[]` = last attempt succeeded (including a
  // genuine empty result, which should NOT retry - see `handleProviderChange`).
  const [modelsByProvider, setModelsByProvider] = useState<Partial<Record<OverrideProvider, string[] | null>>>({});
  const [isLoadingModels, setIsLoadingModels] = useState(false);

  const availableProviders = [
    ...ALWAYS_AVAILABLE_PROVIDERS,
    ...KEYED_PROVIDERS.filter(p => !!providerApiKeys[p]),
  ];

  const loadModelsForProvider = useCallback(async (provider: OverrideProvider) => {
    setIsLoadingModels(true);
    try {
      const apiKey = isKeyedProvider(provider) ? providerApiKeys[provider] : null;
      const names = await fetchModelNames(provider, apiKey);
      setModelsByProvider(prev => ({ ...prev, [provider]: names }));
    } catch (err) {
      console.error(`[LiveActionChipModelPicker] Failed to load ${provider} models:`, err);
      // `null`, not `[]` - a fetch failure must not be indistinguishable from
      // a real "provider has zero models" result, or it'd permanently block
      // retries for the rest of this component's mounted lifetime.
      setModelsByProvider(prev => ({ ...prev, [provider]: null }));
    } finally {
      setIsLoadingModels(false);
    }
  }, [providerApiKeys]);

  const handleProviderChange = (value: string) => {
    if (!isOverrideProvider(value)) return;
    setDraftProvider(value);
    // Falsy covers both `undefined` (never attempted) and `null` (previous
    // attempt failed) - both should retry. A `string[]` is always truthy
    // here, even when empty, so a genuine "provider has zero models" result
    // correctly does NOT retry.
    if (!modelsByProvider[value]) {
      loadModelsForProvider(value);
    }
  };

  const handleModelSelect = (modelName: string) => {
    if (!draftProvider) return;
    onOverrideChange({ provider: draftProvider, modelName });
    setModelPopoverOpen(false);
  };

  const handleReset = () => {
    onOverrideChange(null);
    setDraftProvider(null);
    setOpen(false);
  };

  const draftModels = draftProvider ? modelsByProvider[draftProvider] ?? [] : [];

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant={override ? 'default' : 'outline'}
          size="icon"
          className={cn('rounded-full h-9 w-9', !override && 'glass-pill')}
          title={override ? `Live chip model: ${providerLabel(override.provider)} · ${override.modelName}` : 'Choose live chip model'}
        >
          <SlidersHorizontal className="w-4 h-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-72 space-y-3 text-sm" align="end" sideOffset={8}>
        <div>
          <p className="text-xs font-medium text-muted-foreground mb-1">Live chip model</p>
          <p className="text-xs text-muted-foreground">
            {override
              ? `Overriding: ${providerLabel(override.provider)} · ${override.modelName}`
              : `Using Settings default (${modelConfig.provider} · ${modelConfig.model || 'default'})`}
          </p>
        </div>

        <Select value={draftProvider ?? undefined} onValueChange={handleProviderChange}>
          <SelectTrigger className="h-8 text-xs">
            <SelectValue placeholder="Select provider" />
          </SelectTrigger>
          <SelectContent>
            {availableProviders.map(provider => (
              <SelectItem key={provider} value={provider}>
                {providerLabel(provider)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        {draftProvider && (
          <Popover open={modelPopoverOpen} onOpenChange={setModelPopoverOpen} modal={true}>
            <PopoverTrigger asChild>
              <Button
                variant="outline"
                role="combobox"
                aria-expanded={modelPopoverOpen}
                className="w-full justify-between font-normal h-8 text-xs"
              >
                <span className="truncate">
                  {draftProvider === override?.provider ? override.modelName : 'Select model...'}
                </span>
                <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-[240px] p-0" align="start">
              <Command>
                <CommandInput placeholder="Search models..." />
                <CommandList className="max-h-[240px]">
                  {isLoadingModels ? (
                    <div className="py-6 text-center text-sm text-muted-foreground">
                      <RefreshCw className="mx-auto h-4 w-4 animate-spin mb-2" />
                      Loading models...
                    </div>
                  ) : (
                    <>
                      <CommandEmpty>No models found.</CommandEmpty>
                      <CommandGroup>
                        {draftModels.map(model => (
                          <CommandItem key={model} value={model} onSelect={handleModelSelect}>
                            <Check
                              className={cn(
                                'mr-2 h-4 w-4',
                                draftProvider === override?.provider && override?.modelName === model
                                  ? 'opacity-100'
                                  : 'opacity-0'
                              )}
                            />
                            <span className="truncate">{model}</span>
                          </CommandItem>
                        ))}
                      </CommandGroup>
                    </>
                  )}
                </CommandList>
              </Command>
            </PopoverContent>
          </Popover>
        )}

        {override && (
          <Button type="button" variant="ghost" size="sm" className="w-full text-xs h-7" onClick={handleReset}>
            Reset to Settings default
          </Button>
        )}
      </PopoverContent>
    </Popover>
  );
}
