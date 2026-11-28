import { memo, useMemo } from 'react';
import { useT } from '../lib/I18nContext';
import { fenceLanguage, highlightBlock } from '../lib/diff-syntax';
import { CopyButton } from './ui/CopyButton';
import './CodeBlock.css';

/** KT-1902 — a fenced code block of a message: its language written above it
 *  when known, a Copy button named after that language, the code scrolling
 *  sideways inside the block, coloured by the code tokens (KT-1878). */
export const CodeBlock = memo(function CodeBlock({ code, language }: { code: string; language: string | null }) {
  const { t } = useT();
  const fence = fenceLanguage(language);
  const html = useMemo(() => highlightBlock(code, fence?.id ?? null), [code, fence?.id]);
  const copyLabel = fence ? t('disc.code.copyLanguage', fence.label) : t('disc.code.copy');
  return (
    <div className="kr-code-block" data-language={fence?.id ?? fence?.label ?? undefined}>
      <div className="kr-code-block-bar">
        {fence && <span className="kr-code-block-language">{fence.label}</span>}
        <CopyButton label={copyLabel} text={code} size="sm" className="kr-code-block-copy" />
      </div>
      {/* Focusable so a keyboard can scroll a long line into view. */}
      <pre className="kr-code-block-pre" tabIndex={0}>
        {html !== null
          ? <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
          : <code>{code}</code>}
      </pre>
    </div>
  );
});
