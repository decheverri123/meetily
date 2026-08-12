export interface TokenUsage {
  id: number;
  meetingId: string | null;
  provider: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  estimatedCostUsd: number | null;
  purpose: string;
  createdAt: string;
  metadata: string | null;
}

export interface ModelAggregate {
  provider: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  callCount: number;
}

export interface TimeBucketAggregate {
  bucketStart: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  callCount: number;
}

export interface ModelPricing {
  model: string;
  provider: string;
  promptPricePerMillion: number | null;
  completionPricePerMillion: number | null;
  matchedOpenrouterId: string | null;
  source: string;
}
