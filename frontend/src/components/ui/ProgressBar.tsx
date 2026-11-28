import { useId, type CSSProperties } from 'react';
import { useT } from '../../lib/I18nContext';
import type { ProgressSize, ProgressTone } from './dataKinds';
import './visuallyHidden.css';
import './ProgressBar.css';

export interface ProgressSegment {
  id: string;
  label: string;
  /** A count, not a share: the share is computed. */
  value: number;
  tone: ProgressTone;
}

interface ProgressBase {
  /** What progresses ("Steps done"), or what the parts are of ("Tasks").
   *  Required: it names the bar. */
  label: string;
  /** Keep the label for assistive technology only. */
  hideLabel?: boolean;
  size?: ProgressSize;
  className?: string;
}

export interface SingleProgressProps extends ProgressBase {
  /** Missing: an indeterminate bar (something runs, its end unknown). */
  value?: number;
  /** Default 100. */
  max?: number;
  /** The reading, written beside the label and read for the value: "3/8",
   *  "45 %", "1.2 GB of 4 GB". */
  valueText?: string;
  tone?: ProgressTone;
  segments?: never;
  legend?: never;
}

export interface StackedProgressProps extends ProgressBase {
  /** The parts, in order. */
  segments: readonly ProgressSegment[];
  /** What the shares are of; default the sum of the segments. */
  max?: number;
  /** A list of the parts and their counts under the bar. */
  legend?: boolean;
  value?: never;
  valueText?: never;
  tone?: never;
}

export type ProgressBarProps = SingleProgressProps | StackedProgressProps;

const clamp = (n: number, min: number, max: number) => Math.min(max, Math.max(min, n));
/** A share of the track, in percent, for a custom property. */
const percent = (value: number, total: number) => `${total > 0 ? Number(((value * 100) / total).toFixed(3)) : 0}%`;
const width = (value: string) => ({ '--ui-progress-value': value }) as CSSProperties;

/** KT-1886 — progress towards an end, or the parts of a whole. A single bar
 *  is a `progressbar` (no value: indeterminate); the stacked form is an
 *  image whose name lists every part and its share, so it is read whole —
 *  unless a legend is shown: the legend is then the readable list (named by
 *  the label) and the image is named by the label alone, so the breakdown is
 *  not heard twice. */
export function ProgressBar(props: ProgressBarProps) {
  const { label, hideLabel = false, size = 'md', className } = props;
  const { locale } = useT();
  const labelId = useId();
  const classes = ['ui-progress', className].filter(Boolean).join(' ');

  if (props.segments) {
    const { segments, legend = false } = props;
    const sum = segments.reduce((n, s) => n + Math.max(0, s.value), 0);
    const total = props.max ?? sum;
    const format = new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 0 });
    const share = (value: number) => {
      const ratio = total > 0 ? Math.max(0, value) / total : 0;
      return ratio > 0 && ratio < 0.005 ? `<${format.format(0.01)}` : format.format(ratio);
    };
    const name = legend
      ? label
      : `${label}: ${segments.map(s => `${s.label} ${s.value} (${share(s.value)})`).join(', ')}`;
    return (
      <div className={classes} data-size={size} data-stacked="true">
        {!hideLabel && (
          <div className="ui-progress-head" aria-hidden="true">
            <span className="ui-progress-label">{label}</span>
          </div>
        )}
        <div className="ui-progress-track" role="img" aria-label={name}>
          {segments.filter(s => s.value > 0).map(s => (
            <span key={s.id} className="ui-progress-segment" data-tone={s.tone} style={width(percent(s.value, total))} />
          ))}
        </div>
        {legend && (
          <ul className="ui-progress-legend" aria-label={label}>
            {segments.map(s => (
              <li key={s.id}>
                <span className="ui-progress-swatch" data-tone={s.tone} aria-hidden="true" />
                {s.label}
                <span className="ui-progress-count">{s.value}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    );
  }

  const { value, max = 100, valueText, tone = 'accent' } = props;
  const indeterminate = value === undefined || Number.isNaN(value);
  const now = indeterminate ? undefined : clamp(value, 0, max);
  return (
    <div
      className={classes}
      data-size={size}
      data-tone={tone}
      data-indeterminate={indeterminate ? 'true' : undefined}
    >
      <div className={hideLabel && !valueText ? undefined : 'ui-progress-head'}>
        <span id={labelId} className={hideLabel ? 'ui-progress-label ui-visually-hidden' : 'ui-progress-label'}>{label}</span>
        {valueText && !indeterminate && <span className="ui-progress-reading" aria-hidden="true">{valueText}</span>}
      </div>
      <div
        className="ui-progress-track"
        role="progressbar"
        aria-labelledby={labelId}
        aria-valuemin={indeterminate ? undefined : 0}
        aria-valuemax={indeterminate ? undefined : max}
        aria-valuenow={now}
        aria-valuetext={indeterminate ? undefined : valueText}
      >
        <span className="ui-progress-fill" style={indeterminate ? undefined : width(percent(now ?? 0, max))} />
      </div>
    </div>
  );
}
