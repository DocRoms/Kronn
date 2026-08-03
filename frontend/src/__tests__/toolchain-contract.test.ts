import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, it } from 'vitest';

type PackageManifest = {
  scripts: Record<string, string>;
  devDependencies: Record<string, string>;
};

const manifest = JSON.parse(
  readFileSync(resolve(process.cwd(), 'package.json'), 'utf8'),
) as PackageManifest;

describe('TypeScript toolchain contract', () => {
  it('uses stable TypeScript 7 for builds and keeps TypeScript 6 for API consumers', () => {
    expect(manifest.devDependencies['@typescript/native']).toMatch(
      /^npm:typescript@\^7\./,
    );
    expect(manifest.devDependencies.typescript).toMatch(
      /^npm:@typescript\/typescript6@\^6\./,
    );
    expect(manifest.scripts.build).toMatch(/^tsc\b/);
    expect(manifest.scripts['typecheck:native']).toMatch(/^tsc\b/);
    expect(manifest.scripts['typecheck:legacy']).toMatch(/^tsc6\b/);
  });
});
