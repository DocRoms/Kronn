export type ModelTier = 'cpu' | 'mid' | 'power';

export interface SuggestedModel {
  name: string;
  size: string;
  tier: ModelTier;
  descKey: string;
}

export const SUGGESTED_MODELS: SuggestedModel[] = [
  { name: 'llama3.2:1b', size: '~1.3 GB', tier: 'cpu', descKey: 'ollama.model.llama32_1b' },
  { name: 'llama3.2', size: '~2 GB', tier: 'cpu', descKey: 'ollama.model.llama32' },
  { name: 'qwen3:4b', size: '~2.5 GB', tier: 'cpu', descKey: 'ollama.model.qwen3_4b' },
  { name: 'qwen2.5-coder:14b', size: '~9 GB', tier: 'mid', descKey: 'ollama.model.qwen25coder' },
  { name: 'gemma3:27b', size: '~17 GB', tier: 'power', descKey: 'ollama.model.gemma3_27b' },
  { name: 'qwen3:30b', size: '~19 GB', tier: 'power', descKey: 'ollama.model.qwen3_30b' },
];
