import { Wrench } from 'lucide-react';
import { useT } from '../../lib/I18nContext';

interface Props {
  /** The agent or connection this switch belongs to, for the accessible name. */
  subject: string;
  /** On is the default: the Kronn tool executor reaches the model. */
  checked: boolean;
  onChange: (next: boolean) => void;
  testId?: string;
  disabled?: boolean;
  /** `field` also prints the hint under the switch; `pill` keeps it in the tooltip. */
  variant?: 'pill' | 'field';
}

/** Kronn tools on an HTTP agent or connection: on by default, an off switch for
 *  a model that cannot take tool calls (KT-1600). */
export function ToolsSwitch({ subject, checked, onChange, testId, disabled = false, variant = 'pill' }: Props) {
  const { t } = useT();
  return (
    <span className="set-tools-switch-wrap" data-variant={variant}>
      <label className="set-tools-switch" title={t('agents.toolsHint')} data-on={checked}>
        <Wrench size={12} aria-hidden="true" />
        <span>{t('agents.tools')}</span>
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          aria-label={t('agents.toolsFor', subject)}
          data-testid={testId}
          onChange={event => onChange(event.target.checked)}
        />
      </label>
      {variant === 'field' ? <small className="set-hint">{t('agents.toolsHint')}</small> : null}
    </span>
  );
}
