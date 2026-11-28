import { useContext, useState, type ReactNode } from 'react';
import { ChevronDown } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { Button } from '../ui/Button';
import { SectionHeader } from '../ui/SectionHeader';
import { SettingsFoldContext } from './settingsFoldContext';
import '../ui/visuallyHidden.css';
import './SettingsFold.css';

interface SettingsFoldProps {
  /** Its anchor and the key of its open state (`settings-usage`, `settings-debug-logs`). */
  id: string;
  title: ReactNode;
  /** The line under the title. */
  description?: ReactNode;
  /** The element carries the anchor itself: a section that is a fold. */
  anchored?: boolean;
  className?: string;
  children: ReactNode;
}

/**
 * KT-1923 — a part of Settings folded until the reader opens it: the model
 * catalogue, the usage, the API call log, the debug log viewer. Its header
 * names it and carries the fold button; the content is mounted on the first
 * opening and kept afterwards, so folding it again loses nothing.
 */
export function SettingsFold({ id, title, description, anchored = false, className, children }: SettingsFoldProps) {
  const { t } = useT();
  const shared = useContext(SettingsFoldContext);
  const [own, setOwn] = useState(false);
  const open = shared ? shared.isOpen(id) : own;
  const setOpen = (next: boolean) => (shared ? shared.setOpen(id, next) : setOwn(next));
  const [mounted, setMounted] = useState(open);
  if (open && !mounted) setMounted(true);
  const bodyId = `${id}-content`;
  return (
    <div
      id={anchored ? id : undefined}
      className={className ? `set-fold ${className}` : 'set-fold'}
      data-open={open}
      data-fold={id}
    >
      <SectionHeader
        className="set-fold-header"
        title={title}
        description={description}
        actions={(
          <Button
            variant="ghost"
            size="sm"
            icon={<ChevronDown className="set-fold-chevron" />}
            aria-expanded={open}
            aria-controls={bodyId}
            data-testid={`fold-${id}`}
            onClick={() => setOpen(!open)}
          >
            {open ? t('settings.fold.hide') : t('settings.fold.show')}
            <span className="ui-visually-hidden"> {title}</span>
          </Button>
        )}
      />
      <div id={bodyId} className="set-fold-content" hidden={!open}>
        {mounted && children}
      </div>
    </div>
  );
}
