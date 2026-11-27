// KT-1881 — axe-core inside vitest, on the happy-dom document.
//
// axe-core is not a direct dependency: it ships with @axe-core/playwright,
// already a devDependency (the e2e a11y specs). It is resolved from that
// package, so the unit tests and the e2e specs run the same axe version.
import { createRequire } from 'node:module';

interface AxeViolation {
  id: string;
  impact?: string | null;
  help: string;
  nodes: { target: unknown[] }[];
}
interface AxeResults { violations: AxeViolation[]; passes: { id: string }[] }
interface Axe {
  version: string;
  run: (context: Element, options: Record<string, unknown>) => Promise<AxeResults>;
}

const require = createRequire(import.meta.url);
const axe = createRequire(require.resolve('@axe-core/playwright'))('axe-core') as Axe;

/** WCAG 2.x A and AA rules, and axe's best practices (aria-dialog-name is
 *  one: a dialog without a name passes the WCAG tags). Colour contrast needs layout and computed colours,
 *  which happy-dom does not give: it is measured on the tokens instead
 *  (src/styles/__tests__/themeContrast.test.ts). */
export async function runAxe(context: Element = document.body): Promise<AxeResults> {
  return axe.run(context, {
    runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'best-practice'] },
    rules: { 'color-contrast': { enabled: false } },
  });
}

/** One line per violation, so a failing expectation names the rule and the node. */
export function describeViolations(results: AxeResults): string[] {
  return results.violations.map(v => `${v.id} (${v.impact ?? 'n/a'}): ${v.help} — ${v.nodes.map(n => n.target.join(' ')).join(', ')}`);
}
