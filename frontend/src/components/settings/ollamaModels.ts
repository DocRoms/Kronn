// What the Ollama card suggests downloading (KT-930).
//
// Every entry is an exact tag, pulled as written, and carries the `source` it
// was taken from. A tag is added here only when something in this repository
// shows it exists AND ran: a dated measurement under `docs/research/` or the
// catalogue seed in the backend. Never from memory, and never a guess at a
// sibling tag ("surely there is a plain `gemma4:12b`"): a name that does not
// exist in the Ollama library is a download that fails in front of the user.
//
// What the list does NOT hold: sizes. A figure written here is a figure that
// goes stale or was never measured; the real size is shown by the download's
// own progress and by the installed list once the model is on disk.
//
// Re-check the tags against https://ollama.com/library before a release. The
// procedure is in docs/operations/ollama-local-models.md.

export type ModelTier = 'cpu' | 'mid' | 'power';

export interface SuggestedModel {
  /** The exact Ollama tag. */
  name: string;
  tier: ModelTier;
  descKey: string;
  /** An Apple MLX build: offered only where the backend says it will run. */
  mlx?: boolean;
  /** Where in this repository the tag was seen to exist and run. */
  source: string;
}

/** One per hardware tier, for every machine. */
export const SUGGESTED_MODELS: SuggestedModel[] = [
  {
    name: 'qwen3.5:4b',
    tier: 'cpu',
    descKey: 'ollama.model.qwen35_4b',
    source: 'docs/research/native-tool-catalogue-2026-09-22.md',
  },
  {
    name: 'qwen3:8b',
    tier: 'mid',
    descKey: 'ollama.model.qwen3_8b',
    source: 'backend/src/core/model_catalog/mod.rs (Ollama default seed); docs/operations/ollama-local-models.md (bench)',
  },
  {
    name: 'qwen3:30b-a3b',
    tier: 'power',
    descKey: 'ollama.model.qwen3_30b_a3b',
    source: 'backend/src/core/model_catalog/mod.rs (Ollama reasoning seed); docs/operations/ollama-local-models.md (bench)',
  },
];

/** Offered on top of the list when the backend reports `mlx_capable`: a Mac on
 *  Apple Silicon running an Ollama that runs MLX. One per tier that has one. */
export const MLX_SUGGESTED_MODELS: SuggestedModel[] = [
  {
    name: 'gemma4:12b-mlx',
    tier: 'mid',
    descKey: 'ollama.model.gemma4_12b_mlx',
    mlx: true,
    source: 'docs/research/native-tool-catalogue-2026-09-22.md',
  },
  {
    name: 'qwen3.8:27b-mlx',
    tier: 'power',
    descKey: 'ollama.model.qwen38_27b_mlx',
    mlx: true,
    source: 'docs/research/native-qp-litellm-ollama-2026-09-22.md',
  },
];

/** The list to show: the MLX builds first on a machine that runs them, the
 *  same list as everywhere else otherwise. */
export function suggestedModelsFor(mlxCapable: boolean): SuggestedModel[] {
  return mlxCapable ? [...MLX_SUGGESTED_MODELS, ...SUGGESTED_MODELS] : SUGGESTED_MODELS;
}
