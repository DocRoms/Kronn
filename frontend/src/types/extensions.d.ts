// Type extensions for fields added in newer backend versions.
// These augment auto-generated types until `make typegen` is re-run.

// Types not yet in generated.ts (added until next `make typegen`)
export interface DiscoveredKey {
  provider: string;
  source: string;
  suggested_name: string;
  already_exists: boolean;
}

export interface DiscoverKeysResponse {
  discovered: DiscoveredKey[];
  imported_count: number;
}
