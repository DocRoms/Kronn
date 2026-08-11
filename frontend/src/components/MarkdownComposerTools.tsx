import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import ReactMarkdown from 'react-markdown';
import remarkEmoji from 'remark-emoji';
import remarkGfm from 'remark-gfm';
import { CircleHelp, Eye, Pencil, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import './MarkdownComposerTools.css';

const MARKDOWN_REMARK_PLUGINS = [remarkGfm, remarkEmoji];

const MARKDOWN_PATTERNS = [
  { syntax: '**texte**', insertion: '**texte**', labelKey: 'markdown.bold' },
  { syntax: '_texte_', insertion: '_texte_', labelKey: 'markdown.italic' },
  { syntax: '# Titre', insertion: '# Titre', labelKey: 'markdown.heading' },
  { syntax: '> Citation', insertion: '> Citation', labelKey: 'markdown.quote' },
  { syntax: '`code`', insertion: '`code`', labelKey: 'markdown.inlineCode' },
  { syntax: '```js … ```', insertion: '```js\ncode\n```', labelKey: 'markdown.codeBlock' },
  { syntax: '- élément', insertion: '- élément', labelKey: 'markdown.list' },
  { syntax: '[texte](url)', insertion: '[texte](url)', labelKey: 'markdown.link' },
] as const;

const EMOJI_PATTERNS = [
  { shortcode: ':smile:', emoji: '😄' },
  { shortcode: ':rocket:', emoji: '🚀' },
  { shortcode: ':white_check_mark:', emoji: '✅' },
] as const;
const GITHUB_MARKDOWN_DOCS = 'https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax';

export interface MarkdownEditorProps {
  children: ReactNode;
  content?: string;
  sourceId?: string;
  helpTitle?: string;
  helpContent?: ReactNode;
  showEmojiHelp?: boolean;
  embedded?: boolean;
  className?: string;
}

interface PanelPosition {
  top: number;
  left: number;
  width: number;
  maxHeight: number;
  opensAbove: boolean;
}

/**
 * Shared Markdown writing surface.
 *
 * Editing and preview always occupy the same area. Contextual help is the
 * only floating panel, so every composer keeps the same interaction model
 * without growing when preview is enabled.
 */
export function MarkdownEditor({
  children,
  content,
  sourceId,
  helpTitle,
  helpContent,
  showEmojiHelp = true,
  embedded = false,
  className = '',
}: MarkdownEditorProps) {
  const { t } = useT();
  const instanceId = useId();
  const editPanelId = `${instanceId}-edit`;
  const previewPanelId = `${instanceId}-preview`;
  const helpPanelId = `${instanceId}-help`;
  const rootRef = useRef<HTMLDivElement>(null);
  const [view, setView] = useState<'edit' | 'preview'>('edit');
  const [helpOpen, setHelpOpen] = useState(false);
  const [snapshot, setSnapshot] = useState('');
  const [helpPosition, setHelpPosition] = useState<PanelPosition | null>(null);

  const readSource = () => {
    if (content !== undefined) return content;
    if (!sourceId) return '';
    const source = document.getElementById(sourceId);
    return source instanceof HTMLTextAreaElement ? source.value : '';
  };

  const findSource = () => {
    if (sourceId) {
      const source = document.getElementById(sourceId);
      if (source instanceof HTMLTextAreaElement) return source;
    }
    return rootRef.current?.querySelector('textarea') ?? null;
  };

  const insertAtCaret = (insertion: string) => {
    const source = findSource();
    if (!source) return;

    const start = source.selectionStart ?? source.value.length;
    const end = source.selectionEnd ?? start;
    const nextValue = `${source.value.slice(0, start)}${insertion}${source.value.slice(end)}`;
    const valueSetter = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      'value',
    )?.set;
    valueSetter?.call(source, nextValue);
    source.dispatchEvent(new Event('input', { bubbles: true }));

    setView('edit');
    closeHelp();
    requestAnimationFrame(() => {
      source.focus();
      source.setSelectionRange(start + insertion.length, start + insertion.length);
    });
  };

  const updateHelpPosition = useCallback(() => {
    const root = rootRef.current;
    if (!root) return;
    const rect = root.getBoundingClientRect();
    const viewportPadding = 8;
    const gap = 8;
    const width = Math.min(520, window.innerWidth - viewportPadding * 2);
    const aboveSpace = Math.max(0, rect.top - gap - viewportPadding);
    const belowSpace = Math.max(0, window.innerHeight - rect.bottom - gap - viewportPadding);
    const opensAbove = aboveSpace >= 240 || aboveSpace > belowSpace;
    const availableHeight = opensAbove ? aboveSpace : belowSpace;
    setHelpPosition({
      top: opensAbove ? rect.top - gap : rect.bottom + gap,
      left: Math.min(
        Math.max(viewportPadding, rect.right - width),
        window.innerWidth - width - viewportPadding,
      ),
      width,
      maxHeight: Math.max(160, Math.min(520, availableHeight)),
      opensAbove,
    });
  }, []);

  const closeHelp = useCallback(() => {
    setHelpOpen(false);
    setHelpPosition(null);
  }, []);

  const showPreview = () => {
    setSnapshot(readSource());
    setView('preview');
    closeHelp();
  };

  const toggleHelp = () => {
    if (helpOpen) {
      closeHelp();
      return;
    }
    updateHelpPosition();
    setHelpOpen(true);
  };

  useEffect(() => {
    if (!helpOpen) return;
    const closeOnOutsideClick = (event: MouseEvent) => {
      const target = event.target as Node;
      if (rootRef.current?.contains(target)) return;
      if (document.getElementById(helpPanelId)?.contains(target)) return;
      closeHelp();
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') closeHelp();
    };
    window.addEventListener('resize', updateHelpPosition);
    window.addEventListener('scroll', updateHelpPosition, true);
    document.addEventListener('mousedown', closeOnOutsideClick);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      window.removeEventListener('resize', updateHelpPosition);
      window.removeEventListener('scroll', updateHelpPosition, true);
      document.removeEventListener('mousedown', closeOnOutsideClick);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [closeHelp, helpOpen, helpPanelId, updateHelpPosition]);

  const preview = content ?? snapshot;

  return (
    <div
      ref={rootRef}
      className={`md-editor${embedded ? ' md-editor-embedded' : ''}${className ? ` ${className}` : ''}`}
      data-view={view}
    >
      <div className="md-editor-toolbar">
        <div className="md-editor-tabs" role="tablist" aria-label={t('markdown.mode')}>
          <button
            id={`${instanceId}-edit-tab`}
            type="button"
            role="tab"
            aria-selected={view === 'edit'}
            aria-controls={editPanelId}
            onClick={() => {
              setView('edit');
              closeHelp();
            }}
          >
            <Pencil size={12} aria-hidden="true" />
            {t('markdown.edit')}
          </button>
          <button
            id={`${instanceId}-preview-tab`}
            type="button"
            role="tab"
            aria-selected={view === 'preview'}
            aria-controls={previewPanelId}
            onClick={showPreview}
          >
            <Eye size={12} aria-hidden="true" />
            {t('markdown.preview')}
          </button>
        </div>
        <button
          type="button"
          className="md-editor-help-btn"
          data-active={helpOpen}
          aria-expanded={helpOpen}
          aria-controls={helpOpen ? helpPanelId : undefined}
          aria-label={helpTitle ?? t('markdown.help')}
          title={helpTitle ?? t('markdown.help')}
          onClick={toggleHelp}
        >
          <CircleHelp size={13} aria-hidden="true" />
        </button>
      </div>

      <div
        id={editPanelId}
        className="md-editor-edit-pane"
        role="tabpanel"
        aria-labelledby={`${instanceId}-edit-tab`}
        hidden={view !== 'edit'}
      >
        {children}
      </div>
      <div
        id={previewPanelId}
        className="md-editor-preview-pane"
        role="tabpanel"
        aria-labelledby={`${instanceId}-preview-tab`}
        hidden={view !== 'preview'}
      >
        {preview.trim() ? (
          <div className="md-composer-preview">
            <ReactMarkdown remarkPlugins={MARKDOWN_REMARK_PLUGINS}>{preview}</ReactMarkdown>
          </div>
        ) : (
          <p className="md-composer-empty">{t('markdown.previewEmpty')}</p>
        )}
      </div>

      {helpOpen && helpPosition && createPortal(
        <div
          id={helpPanelId}
          className="md-composer-panel md-composer-panel-fixed"
          data-placement={helpPosition.opensAbove ? 'above' : 'below'}
          role="dialog"
          aria-label={helpTitle ?? t('markdown.help')}
          style={{
            top: helpPosition.top,
            left: helpPosition.left,
            width: helpPosition.width,
            maxHeight: helpPosition.maxHeight,
            transform: helpPosition.opensAbove ? 'translateY(-100%)' : undefined,
          }}
        >
          <div className="md-composer-panel-head">
            <strong>{helpTitle ?? t('markdown.helpTitle')}</strong>
            <button type="button" onClick={closeHelp} aria-label={t('common.close')} title={t('common.close')}>
              <X size={12} aria-hidden="true" />
            </button>
          </div>
          {helpContent && <div className="md-composer-context-help">{helpContent}</div>}
          {helpContent && <div className="md-composer-guide-title">{t('markdown.helpTitle')}</div>}
          <div className="md-composer-cheatsheet">
            {MARKDOWN_PATTERNS.map(pattern => (
              <button
                key={pattern.labelKey}
                type="button"
                onClick={() => insertAtCaret(pattern.insertion)}
                title={t('markdown.insertExample')}
                aria-label={`${t('markdown.insertExample')}: ${pattern.syntax}`}
              >
                <code>{pattern.syntax}</code>
                <span>{t(pattern.labelKey)}</span>
              </button>
            ))}
          </div>
          {showEmojiHelp && (
            <div className="md-composer-emoji-help">
              <strong>{t('markdown.emojiTitle')}</strong>
              <p>{t('markdown.emojiHint')}</p>
              <div className="md-composer-emoji-examples">
                {EMOJI_PATTERNS.map(example => (
                  <button
                    key={example.shortcode}
                    type="button"
                    onClick={() => insertAtCaret(example.emoji)}
                    title={t('markdown.insertExample')}
                  >
                    <span aria-hidden="true">{example.emoji}</span>
                    <code>{example.shortcode}</code>
                  </button>
                ))}
              </div>
              <a href={GITHUB_MARKDOWN_DOCS} target="_blank" rel="noreferrer">
                {t('markdown.emojiDocs')}
              </a>
            </div>
          )}
        </div>,
        document.body,
      )}
    </div>
  );
}
