import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validatePublicationSandbox } from './publication-sandbox.mjs';

const fixture = {
  receipt: 'kronn-e2e-sandbox-v1\n',
  backendUrl: 'http://127.0.0.1:61140', devPort: '61372', pid: '12345\n', listeners: '12345\n',
  config: '# Written by scripts/e2e-sandbox-backend.sh. Not a user config.\n[server]\nhost = "127.0.0.1"\nport = 61140\n\n[tokens]\nkeys = []\n',
};

test('accepts only the configured port and exact listener, including duplicate lsof rows', () => {
  validatePublicationSandbox(fixture);
  validatePublicationSandbox({ ...fixture, listeners: '12345\n12345\n' });
  validatePublicationSandbox({ ...fixture, config: fixture.config.split('\n').filter(line => !line.startsWith('#')).join('\n') });
});

for (const change of [
  { receipt: undefined }, { receipt: 'not-created-by-the-launcher' },
  { backendUrl: 'http://127.0.0.1:3140' }, { backendUrl: 'http://127.0.0.1:03140' },
  { backendUrl: 'http://example.test:61140' }, { backendUrl: 'http://127.0.0.1:61141' },
  { backendUrl: 'http://127.0.0.1:61140/proxy' }, { backendUrl: 'http://secret@127.0.0.1:61140' },
  { devPort: '5173' }, { devPort: '05173' }, { devPort: '61140' }, { devPort: undefined },
  { config: fixture.config.replace('127.0.0.1', '0.0.0.0') }, { config: '[server]\nport = 61140' },
  { pid: '' }, { pid: '12345\n67890' }, { listeners: '' }, { listeners: '67890\n' },
  { listeners: '12345\n67890\n' },
]) {
  test(`refuses unsafe or stale sandbox metadata: ${JSON.stringify(change)}`, () => {
    assert.throws(() => validatePublicationSandbox({ ...fixture, ...change }));
  });
}
