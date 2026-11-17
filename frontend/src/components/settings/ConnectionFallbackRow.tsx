import { useRef, useState, type ReactNode } from 'react';
import { AlertTriangle, Plus } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import type { FallbackMode, ConnectionChainView, ConnectionFallbackTarget, ConnectionLinkView } from '../../types/generated';
import { modeKind, type FallbackOption } from '../../lib/fallbackChain';
import { FallbackChainItem, FallbackMenu, FallbackModeLabel } from './FallbackRow';
import './FallbackRow.css';

interface Props {
  /** Prefix of the test ids. */
  testId: string;
  /** The row's mode: `image` or `video`. */
  mode: FallbackMode;
  /** ComfyUI (`local:comfyui`) or the id of the named connection the row belongs to. */
  owner: string;
  /** The model picker (or the resolved model) of the mode: the first element of the row. */
  model: ReactNode;
  /** What the backend resolved for this row, if it has a fallback. */
  view?: ConnectionChainView;
  /** Every image / video source that could take over; the menu keeps the kind of this row's mode. */
  options: FallbackOption[];
  /** Applies (or, with `null`, removes) the fallback; a refusal rejects with its reason. */
  onChange: (next: ConnectionFallbackTarget | null) => Promise<void>;
  /** A click on « via <card> » of an inherited item opens the card that holds it (ComfyUI or a connection). */
  onOpenConnection?: (connection: string) => void;
}

/** Name and transport of a media source, for a pill. */
const nameOf = (options: FallbackOption[], link: ConnectionLinkView): string =>
  options.find((option) => option.connection === link.connection)?.name ?? link.connection;

/** KT-1706 — one media mode of ComfyUI or of an HTTP connection, drawn like the
 *  rows of an agent: the model, « If unavailable: », the numbered fallback, `+ Add a fallback`. The
 *  fallback goes to the same kind of mode only (image to image, video to video),
 *  never to the source itself; nothing is shown as changed until the backend has
 *  stored it, so a refusal leaves the row as it was. */
export function ConnectionFallbackRow({ testId, mode, owner, model, view, options, onChange, onOpenConnection }: Props) {
  const { t } = useT();
  const [menuOpen, setMenuOpen] = useState(false);
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const saving = useRef(false);

  const link = view?.links[0];
  const offered = options.filter((option) =>
    option.connection !== owner && modeKind(option.mode) === modeKind(mode));
  const taken = [owner, ...(view?.links ?? []).map((entry) => entry.connection),
    ...(view?.inherited ?? []).map((entry) => entry.connection)];

  const apply = async (next: ConnectionFallbackTarget | null) => {
    if (saving.current) return;
    saving.current = true;
    setBusy(true);
    try {
      await onChange(next);
      setRefusal(null);
    } catch (error) {
      setRefusal(error instanceof Error ? error.message : String(error));
    } finally {
      saving.current = false;
      setBusy(false);
    }
  };
  const menu = (selected?: ConnectionLinkView) => (
    <FallbackMenu
      testId={`${testId}-menu`}
      options={offered}
      anchor={anchor}
      taken={taken}
      selected={selected && { agent: 'Custom', connection: selected.connection, model: selected.model ?? null, mode }}
      replaced={mode}
      onPick={(option) => {
        setMenuOpen(false);
        void apply({ connection: option.connection ?? '', model: option.model });
      }}
      onClose={() => setMenuOpen(false)}
    />
  );
  const anchorRef = (element: HTMLElement | null) => {
    if (menuOpen && element) setAnchor((previous) => (previous === element ? previous : element));
  };

  const own = link && (
    <FallbackChainItem
      testId={`${testId}-link-0`}
      position={1}
      tone="own"
      agent="Custom"
      name={nameOf(options, link)}
      model={link.model ?? null}
      mode={mode}
      transport={link.transport}
      title={t('fallback.change')}
      disabled={busy}
      onClick={() => setMenuOpen((open) => !open)}
      anchorRef={anchorRef}
      // One own link per row: nothing to reorder, and the add button is gone while it is set.
      controls={{
        onRemove: () => void apply(null),
        onChange: () => setMenuOpen((open) => !open),
      }}
    >
      {menuOpen && menu(link)}
    </FallbackChainItem>
  );
  const inherited = view?.inherited ?? [];

  return (
    <div className="fbr" data-testid={testId} data-mode={mode} data-kind={modeKind(mode)} data-with-model="true">
      <FallbackModeLabel mode={mode} />
      <div className="fbr-track">
        <span className="fbr-model" data-testid={`${testId}-model`}>{model}</span>
        <span className="fbr-caption">{t('fallback.ifUnavailable')}</span>
        {(link || inherited.length > 0) && (
          <ol className="fbc-list" data-testid={`${testId}-chain`}>
            {own}
            {inherited.map((entry, index) => (
              <FallbackChainItem
                key={`inherited-${entry.connection}-${index}`}
                testId={`${testId}-inherited-${index}`}
                position={(link ? 1 : 0) + index + 1}
                tone="inherited"
                agent="Custom"
                name={nameOf(options, entry)}
                model={entry.model ?? null}
                mode={mode}
                transport={entry.transport}
                title={t('fallback.inheritedHint')}
                via={{
                  name: nameOf(options, entry),
                  onOpen: onOpenConnection ? () => onOpenConnection(entry.connection) : undefined,
                }}
              />
            ))}
          </ol>
        )}
        {!link && inherited.length === 0 && (
          <span className="fbr-off" data-testid={`${testId}-none`}>{t('fallback.none')}</span>
        )}
        {!link && (
          <div className="fbr-foot">
            <span className="fbr-anchor" ref={anchorRef}>
              <button
                type="button"
                className="fbr-add"
                disabled={busy}
                data-testid={`${testId}-add`}
                onClick={() => setMenuOpen((open) => !open)}
              >
                <Plus size={11} aria-hidden="true" />
                {t('fallback.add')}
              </button>
              {menuOpen && menu()}
            </span>
          </div>
        )}
      </div>
      {(view?.error || refusal) && (
        <div className="fbr-error" role="alert" data-testid={`${testId}-refused`}>
          <AlertTriangle size={11} aria-hidden="true" />
          {refusal ? t('fallback.refused', refusal) : view?.error}
        </div>
      )}
    </div>
  );
}
