export const PROVIDER_LABELS: Record<string, string> = {
  openai: 'OpenAI',
  claude: 'Claude',
  groq: 'Groq',
  ollama: 'Ollama',
  openrouter: 'OpenRouter',
  'builtin-ai': 'Built-in AI',
  'custom-openai': 'Custom OpenAI',
  lmstudio: 'LM Studio',
};

export function providerLabel(provider: string): string {
  return PROVIDER_LABELS[provider] ?? provider;
}
