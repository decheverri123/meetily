'use client';
import { useCallback, useMemo, useState, useRef, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { formatDistanceToNow } from 'date-fns';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';
import { providerLabel } from '@/lib/providerLabels';
import { useConfig } from '@/contexts/ConfigContext';
import type { TokenUsage, ModelAggregate, TimeBucketAggregate, ModelPricing } from './types';

function ProviderIcon({ provider, className }: { provider: string; className?: string }) {
  if (provider !== 'ollama') return null;
  return (
    <svg viewBox="0 0 512 512" className={cn('h-3.5 w-3.5', className)} aria-hidden="true">
      <path
        fill="currentColor"
        d="M157.3 35.9c-4.3.7-9.5 3-13.1 5.7c-11 8.4-19.6 26.2-23.2 48.4c-1.4 8.4-2.3 20-2.3 28.9c0 10.5 1.2 23.9 3 33.1c.4 2.1.6 3.9.4 4c-.1.1-1.8 1.5-3.6 2.9c-6.2 5-13.4 12.6-18.3 19.6c-9.4 13.4-15.5 28.6-18.1 45c-1 6.5-1.3 19.6-.5 26.1c1.8 15 6.3 27.7 14 39.3l2.5 3.7l-.7 1.2c-5.2 8.7-9.6 21.3-11.6 33.3c-1.6 9.6-1.8 12.1-1.8 24.9c0 12.9.2 15.5 1.7 24.4c1.8 10.7 5.5 22 9.7 29.5c1.4 2.5 4.7 7.6 5.1 7.8c.1.1-.3 1.3-.9 2.7c-4.8 10.5-8.9 24.4-10.6 36.1c-1.2 8-1.4 10.6-1.4 19.1c0 10.8.6 16 2.9 24.6l.3 1.3h28.4l-.9-1.8c-5.7-10.6-6.3-30.3-1.3-50c2.3-9.1 4.8-15.8 9.6-24.9l2.9-5.6v-3.4c0-3.2-.1-3.5-1.1-5.6c-.8-1.6-1.9-3-3.7-4.8c-3.2-3.1-5.5-6.4-7.4-10.5c-8.2-17.7-9.8-44-4-66.5c2.4-9.4 6.3-17.7 10.5-22.2c2.8-3.1 4.3-6.6 4.3-10.2c0-3.7-1.3-6.8-4.3-10.1c-8.6-9.2-13.8-20.3-15.7-33.3c-2.7-18.5 2.2-38.6 13.3-54.6c10.8-15.7 26.1-25.7 43.1-28.4c3.8-.6 10.9-.5 14.9.2c4.3.8 7.1.5 9.9-.8c3.5-1.6 5.2-3.6 7.2-8.3c1.8-4.1 3.2-6.4 6.9-11.1c4.5-5.6 8.9-9.4 15.8-14c8-5.2 17-9 26-10.8c3.3-.7 4.8-.8 10.9-.8s7.7.1 10.9.8c13.2 2.7 26.4 9.5 36.9 19.2c2.3 2.1 7.7 8.8 9.4 11.6c.7 1.1 1.8 3.4 2.6 5.1c2 4.6 3.7 6.7 7.2 8.3c2.7 1.3 5.5 1.6 9.7.9c6.6-1.1 11.7-1 18.1.3c22 4.4 41.2 22.6 49.7 46.9c7.4 21.3 5.3 43.7-5.7 60.7c-1.9 2.9-3.7 5.2-6.4 8.1c-5.8 6.2-5.8 13.9 0 20.3c9.5 10.4 15.4 35.9 13.6 58.5c-1.2 14.9-5 28.2-10.3 35.7c-.9 1.3-2.9 3.6-4.3 5c-1.9 1.9-3 3.2-3.7 4.8c-1 2.1-1.1 2.5-1.1 5.6v3.4l2.9 5.6c4.8 9.2 7.3 15.9 9.6 24.9c4.9 19.4 4.4 38.7-1.1 49.7c-.5.9-.9 1.8-.9 1.9s6.3.2 14.1.2h14.1l.4-1.4c.2-.8.5-1.9.7-2.6c.4-1.5 1.1-5.8 1.7-9.9c.6-4.2.6-19.6 0-24.2c-2.1-16.9-5.7-30.2-11.5-42.9c-.6-1.4-1-2.7-.9-2.7c.2-.1 1.1-1.4 2.1-2.9c7.2-10.9 11.7-24.7 13.9-42.9c.6-5 .6-26.5 0-31.4c-1.6-12.4-3.5-20.8-6.7-29.4c-1.3-3.5-4.8-11-6.3-13.5l-.7-1.2l2.5-3.7c7.7-11.6 12.2-24.3 14-39.3c.8-6.5.5-19.6-.5-26.1c-2.6-16.5-8.7-31.6-18.1-45c-4.9-7-12-14.7-18.3-19.6c-1.8-1.5-3.5-2.8-3.6-2.9c-.2-.1 0-2 .4-4c4-20.9 3.9-47-.3-67.4c-3.6-17.8-10.3-31.9-18.8-40.1c-6.8-6.5-13.8-9.3-22.2-8.8c-19.2 1.1-34.6 23.2-40.7 58c-1 5.6-1.9 12.2-1.9 14c0 .7-.1 1.3-.3 1.3s-1.5-.7-2.9-1.5C288.5 98.8 272 94.1 256 94.1s-32.5 4.7-47.3 13.4c-1.4.8-2.7 1.5-2.9 1.5s-.3-.6-.3-1.3c0-1.9-.9-8.6-1.9-14c-5.5-31.2-18.2-51.9-35.1-57.1c-2.2-.6-8.8-1.1-11.2-.7m5.6 27c4.8 3.8 10.1 14.6 13.1 26.7c.6 2.2 1.2 4.7 1.3 5.6s.5 2.9.8 4.5c1.3 7 1.9 14.6 2 23.9v9.1l-2.3 3.4l-2.3 3.4h-5.3c-6.2 0-12.4.8-18.4 2.4c-2.1.5-4.2 1.1-4.6 1.2c-.6.1-.7-.1-1.1-2.8c-2-14.8-1.9-31.1.3-44.7c2.4-15.2 8-28.9 13.4-32.9c1.4-1 1.6-1 3.1.2m189.2-.2c3.3 2.4 6.9 8.9 9.6 17.1c5.4 16.5 6.9 39 4.1 60.5c-.4 2.7-.5 2.9-1.1 2.8c-.4-.1-2.5-.6-4.6-1.2c-5.9-1.6-12.1-2.4-18.4-2.4h-5.3l-2.3-3.4l-2.3-3.4v-9.1c.1-12.9 1.3-22.9 4.1-34.1c3-12 8.4-22.8 13.1-26.6c1.6-1.2 1.8-1.2 3.1-.2"
      />
      <path
        fill="currentColor"
        d="M250.9 229.6c-7.2.7-9.2 1-12.6 1.7c-5.6 1.2-13.1 3.7-18.3 6.3c-18.1 8.9-30.6 23.6-34.4 40.7c-.8 3.4-.9 4.5-.9 10.2c0 5.6.1 6.9.8 10.1c5.1 22.3 25.6 38.8 52.3 41.8c5.8.6 30.7.6 36.5 0c21.4-2.4 39.7-14 48-30.3c2.2-4.3 3.3-7.2 4.2-11.6c.7-3.2.8-4.4.8-10.1s-.1-6.8-.9-10.2c-5.5-24.8-29.6-44.4-59.2-48.1c-3.7-.3-13.8-.7-16.3-.5m12.4 18.1c9.9 1.1 19.8 4.6 27.7 9.9c4.3 2.9 10.3 8.8 12.9 12.7c3.2 4.8 5 9.8 5.8 15.8c.4 2.8.2 4.8-.8 9.3c-1.6 6.6-6.4 13.6-12.9 18.4c-3.1 2.2-9.4 5.4-13.3 6.7c-7.4 2.4-12.2 2.8-29.4 2.7c-11.2-.1-13.2-.2-16.4-.8c-11-2.1-19.7-6.4-26-13.1c-5.1-5.4-7.4-10.3-8.7-18.2c-.6-3.7.5-9.8 2.7-14.9c2.6-6.3 9.4-14.1 16.1-18.5c7.8-5.2 18-8.9 27.4-9.9c3.6-.5 11.2-.5 14.9-.1"
      />
      <path
        fill="currentColor"
        d="M243.3 271.9c-2.5 1.4-4.3 4.8-3.7 7.4c.6 2.8 3 5.5 6.8 7.8c2 1.2 2.2 1.4 2.3 2.6c.1.7-.2 2.8-.6 4.7c-.4 1.8-.7 3.7-.7 4.3c0 1.4 1.4 3.7 2.8 4.9c1.2 1 1.5 1 4.9 1.1c3.2.1 3.8 0 5.1-.6c3.3-1.6 4.1-4.5 2.9-10.1c-1-4.7-.8-5.4 1.7-6.8c2.6-1.5 5.4-4.2 6.2-6c1.6-3.5.1-7.4-3.4-9.3c-.9-.4-1.9-.6-3.5-.6c-2.4 0-4 .6-6.8 2.4l-1.6 1l-1-.6c-4.2-2.5-5-2.8-7.5-2.8c-2 0-3 .1-3.9.6m-80.5-38.5c-5.9 1.9-10.3 6.2-12.5 12.3c-1.1 2.9-1.6 7.5-1.2 10c1.1 5.9 6 11.3 11.5 12.8c7 1.8 12.2.6 16.8-3.9c2.7-2.6 4.1-4.9 5.6-8.6c1.1-2.6 1.1-3.1 1.1-6.8v-4l-1.4-2.9c-2.2-4.5-6.2-7.9-10.9-9.1c-2.5-.6-6.7-.6-9 .2m177.2-.1c-4.5 1.2-8.6 4.6-10.7 9.1l-1.4 2.9v4c0 3.7.1 4.2 1.1 6.8c1.5 3.7 2.9 6 5.6 8.6c4.6 4.6 9.8 5.8 16.8 3.9c4-1.1 8-4.4 10-8.4c1.7-3.4 2.1-5.8 1.5-9.6c-1.2-8.7-6.3-15.1-13.9-17.3c-2.3-.7-6.6-.7-9 0"
      />
    </svg>
  );
}

const CHART_COLORS = [
  'hsl(var(--chart-1))',
  'hsl(var(--chart-2))',
  'hsl(var(--chart-3))',
  'hsl(var(--chart-4))',
  'hsl(var(--chart-5))',
];

const PURPOSE_LABELS: Record<string, string> = {
  summary_chunk: 'Summary chunk',
  summary_combine: 'Summary combine',
  summary_final: 'Summary final',
  template_select: 'Template select',
  qa_meeting: 'Meeting Q&A',
  qa_global: 'Global Q&A',
  qa_live: 'Live Q&A',
  suggest_questions: 'Suggest questions',
  live_insights: 'Live insights',
  live_action_chip: 'Live action chip',
  normalize: 'Normalize',
  translate: 'Translate',
  other: 'Other',
};

function labelFrom(map: Record<string, string>, key: string): string {
  return map[key] ?? key;
}

function purposeLabel(purpose: string): string {
  return labelFrom(PURPOSE_LABELS, purpose);
}

function estimateCostUsd(
  pricing: ModelPricing | undefined,
  promptTokens: number,
  completionTokens: number
): number | null {
  if (!pricing) return null;
  const { promptPricePerMillion, completionPricePerMillion } = pricing;
  if (promptPricePerMillion === null || completionPricePerMillion === null) return null;
  return (
    (promptTokens / 1e6) * promptPricePerMillion +
    (completionTokens / 1e6) * completionPricePerMillion
  );
}

function formatCostUsd(value: number | null): string {
  if (value === null) return '—';
  const digits = value > 0 && value < 0.01 ? 4 : 2;
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(value);
}

function formatTokens(value: number): string {
  return new Intl.NumberFormat().format(value);
}

function safeParseDate(value: string): Date | null {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

function formatRelative(value: string | null): string {
  if (!value) return '—';
  const parsed = safeParseDate(value);
  return parsed ? formatDistanceToNow(parsed, { addSuffix: true }) : '—';
}

// Buckets are truncated to UTC day boundaries by the Rust aggregate, so label
// them in UTC to match - local-time formatting would show the previous day in
// behind-UTC timezones.
function formatShortDateUtc(value: string): string {
  const parsed = safeParseDate(value);
  if (!parsed) return '—';
  return `${parsed.getUTCMonth() + 1}/${parsed.getUTCDate()}`;
}

function purposeBadgeClass(purpose: string): string {
  switch (purpose) {
    case 'summary_chunk':
    case 'summary_combine':
    case 'summary_final':
      return 'bg-primary/15 text-primary';
    case 'qa_meeting':
    case 'qa_global':
    case 'qa_live':
      return 'bg-accent-violet/15 text-accent-violet';
    case 'live_insights':
    case 'live_action_chip':
      return 'bg-success/15 text-success';
    default:
      return 'bg-secondary/15 text-muted-foreground';
  }
}

interface UsageQueryOpts {
  since?: string;
  until?: string;
  provider?: string;
  model?: string;
  purpose?: string;
  meetingId?: string;
  limit?: number;
}

export function TokenUsagePageContent({
  rows: initialRows,
  byModel: initialByModel,
  overTime: initialOverTime,
}: {
  rows: TokenUsage[];
  byModel: ModelAggregate[];
  overTime: TimeBucketAggregate[];
}) {
  const [rows, setRows] = useState<TokenUsage[]>(initialRows);
  const [byModel, setByModel] = useState<ModelAggregate[]>(initialByModel);
  const [overTime, setOverTime] = useState<TimeBucketAggregate[]>(initialOverTime);
  const [provider, setProvider] = useState<string>('all');
  const [model, setModel] = useState<string>('all');
  const [purpose, setPurpose] = useState<string>('all');
  const [since, setSince] = useState<string>('');
  const [until, setUntil] = useState<string>('');
  const [loading, setLoading] = useState(false);
  const skipInitial = useRef(true);
  const requestIdRef = useRef(0);
  const { modelConfig } = useConfig();
  const [pricingByModel, setPricingByModel] = useState<Record<string, ModelPricing>>({});

  const filteredModels = useMemo(() => {
    const all = byModel.map((a) => a.model);
    return Array.from(new Set(all)).sort();
  }, [byModel]);

  const purposes = useMemo(() => {
    const all = rows.map((r) => r.purpose);
    return Array.from(new Set(all)).sort();
  }, [rows]);

  const providers = useMemo(() => {
    const all = byModel.map((a) => a.provider);
    return Array.from(new Set(all)).sort();
  }, [byModel]);

  const totalTokens = useMemo(
    () => rows.reduce((sum, r) => sum + r.totalTokens, 0),
    [rows]
  );

  const estimatedTotalCost = useMemo(
    () =>
      byModel.reduce<number | null>((acc, agg) => {
        const cost = estimateCostUsd(
          pricingByModel[`${agg.provider}:${agg.model}`],
          agg.promptTokens,
          agg.completionTokens
        );
        if (cost === null) return acc;
        return (acc ?? 0) + cost;
      }, null),
    [byModel, pricingByModel]
  );

  useEffect(() => {
    const distinct = Array.from(
      new Map(byModel.map((a) => [`${a.provider}:${a.model}`, a])).values()
    );
    if (distinct.length === 0) return;
    let cancelled = false;
    invoke<ModelPricing[]>('api_resolve_model_pricing', {
      models: distinct.map((a) => ({ model: a.model, provider: a.provider })),
      ollamaEndpoint: modelConfig.ollamaEndpoint ?? null,
    })
      .then((pricing) => {
        if (cancelled) return;
        setPricingByModel(
          Object.fromEntries(pricing.map((p) => [`${p.provider}:${p.model}`, p]))
        );
      })
      .catch((err) => {
        if (!cancelled) toast.error(`Failed to resolve model pricing: ${err}`);
      });
    return () => {
      cancelled = true;
    };
  }, [byModel, modelConfig.ollamaEndpoint]);

  const refetch = useCallback(
    async (filters: {
      provider: string;
      model: string;
      purpose: string;
      since: string;
      until: string;
    }) => {
      setLoading(true);
      const id = ++requestIdRef.current;
      const opts: UsageQueryOpts = { limit: 500 };
      if (filters.provider !== 'all') opts.provider = filters.provider;
      if (filters.model !== 'all') opts.model = filters.model;
      if (filters.purpose !== 'all') opts.purpose = filters.purpose;
      if (filters.since) opts.since = new Date(filters.since).toISOString();
      if (filters.until) opts.until = new Date(filters.until).toISOString();
      try {
        const [newRows, newByModel, newOverTime] = await Promise.all([
          invoke<TokenUsage[]>('api_list_token_usage', { opts }),
          invoke<ModelAggregate[]>('api_aggregate_token_usage_by_model', {
            since: opts.since ?? null,
          }),
          invoke<TimeBucketAggregate[]>('api_aggregate_token_usage_over_time', {
            bucket: 'day',
            since:
              opts.since ??
              new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString(),
          }),
        ]);
        if (id !== requestIdRef.current) return;
        setRows(newRows);
        setByModel(newByModel);
        setOverTime(newOverTime);
      } catch (err) {
        if (id !== requestIdRef.current) return;
        toast.error(`Failed to update token usage: ${err}`);
      } finally {
        if (id === requestIdRef.current) setLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    if (skipInitial.current) {
      skipInitial.current = false;
      return;
    }
    refetch({ provider, model, purpose, since, until });
  }, [provider, model, purpose, since, until, refetch]);

  const chartBuckets = useMemo(() => {
    const buckets = overTime.slice(-30);
    const max = Math.max(1, ...buckets.map((b) => b.totalTokens));
    return buckets.map((b, i) => ({
      ...b,
      color: CHART_COLORS[i % CHART_COLORS.length],
      height: b.totalTokens === 0 ? 2 : Math.max(2, Math.round((b.totalTokens / max) * 160)),
    }));
  }, [overTime]);

  const barWidth = 12;
  const barGap = 4;
  const chartWidth = chartBuckets.length * (barWidth + barGap);
  const chartHeight = 200;
  const axisPadding = 20;

  const recent = useMemo(() => rows.slice(0, 100), [rows]);

  return (
    <div className="relative flex flex-col h-screen overflow-hidden bg-background text-foreground">
      <div className="pointer-events-none absolute inset-0 overflow-hidden">
        <div className="animate-drift absolute -top-1/4 left-1/3 h-[60vh] w-[60vh] rounded-full bg-primary/10 blur-[120px]" />
        <div className="animate-drift absolute bottom-0 right-0 h-[50vh] w-[50vh] rounded-full bg-accent-violet/10 blur-[120px]" style={{ animationDelay: '6s' }} />
      </div>

      <div className="relative z-10 flex flex-1 flex-col overflow-y-auto gap-5 p-6">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Token Usage &amp; Pricing</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            {formatTokens(rows.length)} calls · {formatTokens(totalTokens)} tokens tracked
            {estimatedTotalCost !== null && (
              <> · est. cost {formatCostUsd(estimatedTotalCost)}</>
            )}
          </p>
        </div>

        <div className="flex flex-wrap items-end gap-3 rounded-xl border border-border/10 bg-secondary/5 p-3">
          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">Provider</span>
            <Select value={provider} onValueChange={setProvider}>
              <SelectTrigger className="w-40">
                <SelectValue placeholder="All providers" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All providers</SelectItem>
                {providers.map((p) => (
                  <SelectItem key={p} value={p}>{providerLabel(p)}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">Model</span>
            <Select value={model} onValueChange={setModel}>
              <SelectTrigger className="w-40">
                <SelectValue placeholder="All models" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All models</SelectItem>
                {filteredModels.map((m) => (
                  <SelectItem key={m} value={m}>{m}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">Purpose</span>
            <Select value={purpose} onValueChange={setPurpose}>
              <SelectTrigger className="w-44">
                <SelectValue placeholder="All purposes" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All purposes</SelectItem>
                {purposes.map((p) => (
                  <SelectItem key={p} value={p}>{purposeLabel(p)}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">Since</span>
            <Input
              type="date"
              value={since}
              onChange={(e) => setSince(e.target.value)}
              className="w-40"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">Until</span>
            <Input
              type="date"
              value={until}
              onChange={(e) => setUntil(e.target.value)}
              className="w-40"
            />
          </div>
        </div>

        {loading && (
          <p className="text-sm text-muted-foreground">Updating…</p>
        )}

        <section>
          <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-muted-foreground">
            By Model
          </h2>
          {byModel.length === 0 ? (
            <EmptyState />
          ) : (
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
              {byModel.map((agg) => {
                const lastUsed = rows
                  .filter((r) => r.model === agg.model && r.provider === agg.provider)
                  .reduce<string | null>(
                    (latest, r) =>
                      !latest || r.createdAt > latest ? r.createdAt : latest,
                    null
                  );
                const pricing = pricingByModel[`${agg.provider}:${agg.model}`];
                const cost = estimateCostUsd(pricing, agg.promptTokens, agg.completionTokens);
                return (
                  <Card key={`${agg.provider}:${agg.model}`} className="bg-secondary/[.04]">
                    <CardHeader className="pb-2">
                      <div className="flex items-start justify-between gap-2">
                        <CardTitle className="text-base">{agg.model}</CardTitle>
                        <Badge variant="outline" className="shrink-0">
                          <ProviderIcon provider={agg.provider} className="mr-1" />
                          {providerLabel(agg.provider)}
                        </Badge>
                      </div>
                      <CardDescription className="text-xs">
                        Last used {formatRelative(lastUsed)}
                      </CardDescription>
                    </CardHeader>
                    <CardContent>
                      <div className="flex items-end justify-between">
                        <div>
                          <div className="text-2xl font-semibold tabular-nums">
                            {formatTokens(agg.totalTokens)}
                          </div>
                          <div className="text-xs text-muted-foreground">tokens</div>
                        </div>
                        <div className="text-right text-xs text-muted-foreground">
                          <div className="font-medium text-foreground tabular-nums">
                            {formatTokens(agg.callCount)} calls
                          </div>
                          <div className="tabular-nums">
                            {formatTokens(agg.promptTokens)} in · {formatTokens(agg.completionTokens)} out
                          </div>
                          <div
                            className="tabular-nums"
                            title={
                              cost !== null && pricing?.matchedOpenrouterId
                                ? `est. via ${pricing.matchedOpenrouterId}`
                                : undefined
                            }
                          >
                            est. {formatCostUsd(cost)}
                          </div>
                        </div>
                      </div>
                    </CardContent>
                  </Card>
                );
              })}
            </div>
          )}
        </section>

        <section>
          <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-muted-foreground">
            Over Time
          </h2>
          <Card className="bg-secondary/[.04]">
            <CardContent className="pt-6">
              {chartBuckets.length === 0 ? (
                <EmptyState />
              ) : (
                <div>
                  <svg
                    width="100%"
                    height={chartHeight}
                    viewBox={`0 0 ${chartWidth} ${chartHeight}`}
                    role="img"
                    aria-label="Token usage over time"
                    className="overflow-visible"
                  >
                    {chartBuckets.map((bucket, i) => {
                      const x = i * (barWidth + barGap);
                      const y = chartHeight - axisPadding - bucket.height;
                      return (
                        <rect
                          key={i}
                          x={x}
                          y={y}
                          width={barWidth}
                          height={bucket.height}
                          fill={bucket.color}
                          rx={2}
                        />
                      );
                    })}
                  </svg>
                  <div className="mt-2 flex justify-between text-xs text-muted-foreground">
                    <span>
                      {chartBuckets.length > 0 ? formatShortDateUtc(chartBuckets[0].bucketStart) : ''}
                    </span>
                    <span className="text-[10px] uppercase tracking-wide">UTC</span>
                    <span>
                      {chartBuckets.length > 0
                        ? formatShortDateUtc(chartBuckets[chartBuckets.length - 1].bucketStart)
                        : ''}
                    </span>
                  </div>
                </div>
              )}
            </CardContent>
          </Card>
        </section>

        <section>
          <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-muted-foreground">
            Recent Usage
          </h2>
          {recent.length === 0 ? (
            <EmptyState />
          ) : (
            <div className="space-y-2">
              {recent.map((row) => {
                const pricing = pricingByModel[`${row.provider}:${row.model}`];
                const cost = estimateCostUsd(pricing, row.promptTokens, row.completionTokens);
                return (
                <div
                  key={row.id}
                  className="flex flex-wrap items-center gap-3 rounded-xl border border-border/10 bg-secondary/[.04] px-4 py-3"
                >
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <Badge variant="outline" className="shrink-0">
                        <ProviderIcon provider={row.provider} className="mr-1" />
                        {providerLabel(row.provider)}
                      </Badge>
                      <span className="truncate font-medium">{row.model}</span>
                    </div>
                    <div className="mt-1 text-xs text-muted-foreground">
                      {formatRelative(row.createdAt)}
                    </div>
                  </div>
                  <Badge className={cn('shrink-0', purposeBadgeClass(row.purpose))}>
                    {purposeLabel(row.purpose)}
                  </Badge>
                  <div className="shrink-0 text-right text-xs text-muted-foreground tabular-nums">
                    <div className="font-medium text-foreground">
                      {formatTokens(row.totalTokens)} tokens
                    </div>
                    <div>
                      {formatTokens(row.promptTokens)} in · {formatTokens(row.completionTokens)} out
                    </div>
                    <div
                      title={
                        cost !== null && pricing?.matchedOpenrouterId
                          ? `est. via ${pricing.matchedOpenrouterId}`
                          : undefined
                      }
                    >
                      est. {formatCostUsd(cost)}
                    </div>
                  </div>
                </div>
              );
              })}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function EmptyState() {
  return (
    <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-border/15 bg-secondary/[.035] px-6 py-12 text-center">
      <p className="text-sm font-medium text-foreground">No token usage yet</p>
      <p className="mt-1 max-w-md text-sm text-muted-foreground">
        Token usage starts appearing here as soon as summaries, Q&amp;A, and live
        insights run. Record a meeting or ask a question to populate this page.
      </p>
    </div>
  );
}
