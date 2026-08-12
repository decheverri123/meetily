import { describe, expect, test, afterEach, mock } from 'bun:test';
import { cleanup, render, within } from '@testing-library/react';
import { TokenUsagePageContent } from '../../src/app/token-usage/page-content';
import type { TokenUsage, ModelAggregate, TimeBucketAggregate, ModelPricing } from '../../src/app/token-usage/types';

mock.module('@/contexts/ConfigContext', () => ({
  useConfig: () => ({
    modelConfig: { provider: 'ollama', model: 'llama3.2:latest', ollamaEndpoint: null },
  }),
}));

let invokeImpl: (cmd: string, args?: unknown) => Promise<unknown> = async () => [];
mock.module('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeImpl(cmd, args),
}));

const rows: TokenUsage[] = [
  {
    id: 1,
    meetingId: null,
    provider: 'ollama',
    model: 'llama3.2:latest',
    promptTokens: 100,
    completionTokens: 200,
    totalTokens: 300,
    estimatedCostUsd: null,
    purpose: 'qa_live',
    createdAt: '2026-08-12T12:00:00Z',
    metadata: null,
  },
];

const byModel: ModelAggregate[] = [
  {
    provider: 'ollama',
    model: 'llama3.2:latest',
    promptTokens: 100,
    completionTokens: 200,
    totalTokens: 300,
    callCount: 1,
  },
];

const overTime: TimeBucketAggregate[] = [
  {
    bucketStart: '2026-08-11T00:00:00Z',
    promptTokens: 100,
    completionTokens: 200,
    totalTokens: 300,
    callCount: 1,
  },
  {
    bucketStart: '2026-08-12T00:00:00Z',
    promptTokens: 100,
    completionTokens: 200,
    totalTokens: 300,
    callCount: 1,
  },
];

const pricedRows: TokenUsage[] = [
  {
    id: 2,
    meetingId: null,
    provider: 'openai',
    model: 'gpt-4o-mini',
    promptTokens: 1000000,
    completionTokens: 1000000,
    totalTokens: 2000000,
    estimatedCostUsd: null,
    purpose: 'qa_live',
    createdAt: '2026-08-12T12:00:00Z',
    metadata: null,
  },
];

const pricedByModel: ModelAggregate[] = [
  {
    provider: 'openai',
    model: 'gpt-4o-mini',
    promptTokens: 1000000,
    completionTokens: 1000000,
    totalTokens: 2000000,
    callCount: 1,
  },
];

afterEach(() => {
  cleanup();
});

describe('TokenUsagePageContent', () => {
  test('renders the model name in the By Model section', () => {
    const view = within(render(
      <TokenUsagePageContent rows={rows} byModel={byModel} overTime={overTime} />
    ).container);
    expect(view.getAllByText('llama3.2:latest').length).toBeGreaterThan(0);
  });

  test('renders one rect per time bucket in the chart', () => {
    const { container } = render(
      <TokenUsagePageContent rows={rows} byModel={byModel} overTime={overTime} />
    );
    const rects = container.querySelectorAll('svg rect');
    expect(rects.length).toBe(overTime.length);
  });

  test('lists the row model and purpose in Recent Usage', () => {
    const view = within(render(
      <TokenUsagePageContent rows={rows} byModel={byModel} overTime={overTime} />
    ).container);
    const recentHeading = view.getByText('Recent Usage');
    const recentSection = recentHeading.closest('section');
    expect(recentSection).not.toBeNull();
    const recentView = within(recentSection as HTMLElement);
    expect(recentView.getByText('llama3.2:latest')).not.toBeNull();
    expect(recentView.getByText('Live Q&A')).not.toBeNull();
  });

  test('renders estimated cost for a known model and "—" for an unknown one', async () => {
    const gptPricing: ModelPricing = {
      model: 'gpt-4o-mini',
      provider: 'openai',
      promptPricePerMillion: 0.15,
      completionPricePerMillion: 0.6,
      matchedOpenrouterId: 'openai/gpt-4o-mini',
      source: 'openrouter',
    };
    invokeImpl = async (cmd) => {
      if (cmd === 'api_resolve_model_pricing') return [gptPricing];
      return [];
    };
    const view = within(render(
      <TokenUsagePageContent rows={pricedRows} byModel={pricedByModel} overTime={overTime} />
    ).container);
    // gpt-4o-mini: 1M in @ $0.15 + 1M out @ $0.60 = $0.75
    expect((await view.findAllByText(/est\. \$0\.75/)).length).toBeGreaterThan(0);
  });

  test('renders "—" for a local model with no pricing', async () => {
    invokeImpl = async (cmd) => {
      if (cmd === 'api_resolve_model_pricing') {
        return [{
          model: 'llama3.2:latest',
          provider: 'ollama',
          promptPricePerMillion: null,
          completionPricePerMillion: null,
          matchedOpenrouterId: null,
          source: 'local',
        }];
      }
      return [];
    };
    const view = within(render(
      <TokenUsagePageContent rows={rows} byModel={byModel} overTime={overTime} />
    ).container);
    expect((await view.findAllByText(/est\. —/)).length).toBeGreaterThan(0);
  });

  test('renders chart bucket labels in UTC', () => {
    const view = within(render(
      <TokenUsagePageContent rows={rows} byModel={byModel} overTime={overTime} />
    ).container);
    // 2026-08-11T00:00:00Z and 2026-08-12T00:00:00Z are UTC-midnight buckets;
    // they must render as 8/11 and 8/12 regardless of the local timezone.
    expect(view.getByText('8/11')).not.toBeNull();
    expect(view.getByText('8/12')).not.toBeNull();
  });
});
