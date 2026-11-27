import { useEffect, useState, type CSSProperties } from 'react';
import { Palette } from 'lucide-react';
import { DESIGN_TOKEN_GROUPS, ROOT_FONT_PX, remToPx, type TokenPreview } from '../../styles/designTokens';
import './DesignSystemSection.css';

/** The address of this section: `settingsSectionPath(DESIGN_SYSTEM_SECTION_ID)`. */
export const DESIGN_SYSTEM_SECTION_ID = 'design-system';

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

/** KT-1878 — Settings > Design system: each token of the design scales with
 *  its name, its value in the current theme and one line on its use. Linked
 *  from the appearance settings, absent from the section index. The table it
 *  renders is src/styles/designTokens.ts, held equal to tokens.css by a test. */
export function DesignSystemSection({ t }: Props) {
  const { root, tokens } = useTokenValues();
  return (
    <div id={DESIGN_SYSTEM_SECTION_ID} className="set-card ds-card" data-testid="design-system-section">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <Palette size={14} className="text-accent" />
          <h2 className="ds-title">{t('designSystem.title')}</h2>
        </div>
        <p className="set-hint">{t('designSystem.hint')}</p>
        {DESIGN_TOKEN_GROUPS.map(group => (
          <section key={group.scale} className="ds-group" data-scale={group.scale} aria-labelledby={`ds-${group.scale}`}>
            <h3 id={`ds-${group.scale}`} className="ds-group-title">{t(group.title)}</h3>
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
      </div>
    </div>
  );
}
