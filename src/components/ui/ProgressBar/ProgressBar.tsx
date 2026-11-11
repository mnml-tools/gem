/**
 * A thin square progress track (`mnml-progress`): the soft fill with the indicator colour for
 * the done part, or a sliding segment when the total is unknown. `tone` follows the state of
 * what it measures (danger for a failed transfer). Always labelled for assistive technology.
 */
import './ProgressBar.css';

type Props = {
  /** 0–1, or `null` when the total is unknown. */
  value: number | null;
  label: string;
  tone?: 'default' | 'done' | 'danger' | 'muted';
  className?: string;
};

export function ProgressBar({ value, label, tone = 'default', className }: Props) {
  const percent = value == null ? null : Math.round(Math.min(1, Math.max(0, value)) * 100);
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent ?? undefined}
      className={className ? `mnml-progress ${className}` : 'mnml-progress'}
      data-tone={tone === 'default' ? undefined : tone}
      data-indeterminate={percent == null || undefined}
    >
      <span
        className="mnml-progress-fill"
        style={{ width: percent == null ? undefined : `${percent}%` }}
      />
    </div>
  );
}
