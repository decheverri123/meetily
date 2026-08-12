'use client';
import { Suspense, useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { LoaderIcon } from 'lucide-react';
import { toast } from 'sonner';
import { TokenUsagePageContent } from './page-content';
import type { TokenUsage, ModelAggregate, TimeBucketAggregate } from './types';

async function loadAll(): Promise<{
  rows: TokenUsage[];
  byModel: ModelAggregate[];
  overTime: TimeBucketAggregate[];
}> {
  const [rows, byModel, overTime] = await Promise.all([
    invoke<TokenUsage[]>('api_list_token_usage', { opts: { limit: 500 } }),
    invoke<ModelAggregate[]>('api_aggregate_token_usage_by_model', { since: null }),
    invoke<TimeBucketAggregate[]>('api_aggregate_token_usage_over_time', {
      bucket: 'day',
      since: new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString(),
    }),
  ]);
  return { rows, byModel, overTime };
}

function TokenUsagePageInner() {
  const [data, setData] = useState<{
    rows: TokenUsage[];
    byModel: ModelAggregate[];
    overTime: TimeBucketAggregate[];
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    loadAll()
      .then((result) => {
        if (!cancelled) setData(result);
      })
      .catch((err) => {
        if (!cancelled) toast.error(`Failed to load token usage: ${err}`);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!data) {
    return (
      <div className="flex items-center justify-center h-screen bg-background text-foreground">
        <LoaderIcon className="animate-spin size-6" />
      </div>
    );
  }

  return <TokenUsagePageContent {...data} />;
}

export default function TokenUsagePage() {
  return (
    <Suspense fallback={
      <div className="flex items-center justify-center h-screen bg-background text-foreground">
        <LoaderIcon className="animate-spin size-6" />
      </div>
    }>
      <TokenUsagePageInner />
    </Suspense>
  );
}
