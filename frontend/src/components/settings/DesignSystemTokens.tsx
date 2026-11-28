import { useEffect, useState, type CSSProperties } from 'react';
import { DESIGN_TOKEN_GROUPS, ROOT_FONT_PX, remToPx, type TokenPreview } from '../../styles/designTokens';
import { TOKENS_ANCHOR } from './designSystemGallery';
import './DesignSystemSection.css';

type Values = { root: number; tokens: Record<string, string> };

/** Every token of the table as the current theme computes it, and the root font size. */
function readValues(): Values {
  const style = getComputedStyle(document.documentElement);
  const tokens: Record<string, string> = {};
  for (const group of DESIGN_TOKEN_GROUPS) {
    for (const token of group.tokens) tokens[token.name] = style.getPropertyValue(token.name).trim();
  }
  const root = parseFloat(style.fontSize);
  return { root: Number.isFinite(root) && root > 0 ? root : ROOT_FONT_PX, tokens };
}

/** Re-reads the values whenever the theme, the density or an inline token on <html> changes. */
function useTokenValues(): Values {
  const [values, setValues] = useState(readValues);
  useEffect(() => {
    const refresh = () => setValues(readValues());
    const observer = new MutationObserver(refresh);
    observer.observe(document.documentElement, {
      attributes: true, attributeFilter: ['data-theme', 'data-density', 'style', 'class'],
    });
    refresh();
    return () => observer.disconnect();
  }, []);
  return values;
}

function Preview({ kind, name }: { kind: TokenPreview; name: string }) {
  const ref = `var(${name})`;
  const style: Record<TokenPreview, CSSProperties> = {
    text: { fontSize: ref },
    space: { width: ref },
    radius: { borderRadius: ref },
    // A breakpoint drawn at a tenth of its width, so the three compare.
    width: { width: `calc(${ref} / 10)` },
    duration: { transitionDuration: ref },
    weight: { fontWeight: ref },
    height: { minHeight: ref },
    colour: { background: ref },
  };
  const sample = kind === 'text' || kind === 'weight' ? 'Kronn Aa 012' : null;
  return (
    <span className="ds-preview" data-kind={kind} style={style[kind]} aria-hidden="true">
      {sample}
    </span>
  );
}

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
}

/** KT-1878 — the Tokens tab of the design system: each token of the design
 *  scales with its name, its value in the current theme and one line on its
 *  use, grouped by scale. The table it renders is src/styles/designTokens.ts,
 *  held equal to tokens.css by a test. */
export function DesignSystemTokens({ t }: Props) {
  const { root, tokens } = useTokenValues();
  return (
    <section className="ds-part" aria-labelledby={TOKENS_ANCHOR}>
      {/* The tab says it already: the heading is the index's target and the region's name. */}
      <h2 id={TOKENS_ANCHOR} className="ds-part-title ds-sr-only" tabIndex={-1}>{t('designSystem.tokens')}</h2>
      {DESIGN_TOKEN_GROUPS.map(group => (
        <section key={group.scale} className="ds-group" data-scale={group.scale} aria-labelledby={`ds-${group.scale}`}>
          <h3 id={`ds-${group.scale}`} className="ds-group-title" tabIndex={-1}>{t(group.title)}</h3>
          <table className="ds-table">
            <thead>
              <tr>
                <th scope="col">{t('designSystem.column.token')}</th>
                <th scope="col">{t('designSystem.column.value')}</th>
                <th scope="col">{t('designSystem.column.use')}</th>
                <th scope="col"><span className="ds-sr-only">{t('designSystem.column.preview')}</span></th>
              </tr>
            </thead>
            <tbody>
              {group.tokens.map(token => {
                const value = tokens[token.name] ?? '';
                const px = group.scale === 'text' ? remToPx(value, root) : null;
                return (
                  <tr key={token.name} data-token={token.name}>
                    <th scope="row"><code>{token.name}</code></th>
                    <td className="ds-value" data-testid={`ds-value-${token.name}`}>
                      {value || '—'}
                      {px !== null && <span className="ds-px"> · {Number(px.toFixed(2))}px</span>}
                    </td>
                    <td className="ds-use">{t(token.use)}</td>
                    <td className="ds-preview-cell"><Preview kind={group.preview} name={token.name} /></td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </section>
      ))}
    </section>
  );
}
