/**
 * A vertical split (`mnml-split`): content on top, a resizable pane at the bottom. The kit's
 * `useResize` is width-only, so this is its height twin: drag the hairline, or focus it and use
 * ↑/↓ (Shift for bigger steps), Home and End. The height is remembered under `storageKey`.
 * When `collapsed`, the bottom pane shrinks to its own content (e.g. a header strip).
 */
import { useId, type ReactNode } from 'react';
import { useHeightResize } from './useHeightResize';
import './SplitPane.css';

type Props = {
  top: ReactNode;
  /** `null` hides the bottom pane (the top keeps its place in the tree, so it never remounts). */
  bottom: ReactNode;
  storageKey: string;
  /** Accessible name of the handle, e.g. "Resize bottom panel". */
  label: string;
  initial?: number;
  minimum?: number;
  maximum?: number;
  collapsed?: boolean;
  className?: string;
};

export function SplitPane({
  top,
  bottom,
  storageKey,
  label,
  initial = 200,
  minimum = 80,
  maximum = 600,
  collapsed = false,
  className,
}: Props) {
  const bottomId = useId();
  const resize = useHeightResize({ storageKey, initial, minimum, maximum });
  return (
    <div className={className ? `mnml-split ${className}` : 'mnml-split'}>
      <div className="mnml-split-top">{top}</div>
      {bottom != null && (
        <div
          id={bottomId}
          className="mnml-split-bottom"
          data-collapsed={collapsed || undefined}
          style={collapsed ? undefined : { height: resize.height }}
        >
          {!collapsed && (
            <div
              role="separator"
              aria-label={label}
              aria-controls={bottomId}
              aria-orientation="horizontal"
              aria-valuemin={resize.minimum}
              aria-valuemax={resize.maximum}
              aria-valuenow={resize.height}
              aria-valuetext={`${resize.height} pixels`}
              // A focusable separator is the WAI-ARIA window splitter (the kit's ResizeHandle too).
              // eslint-disable-next-line jsx-a11y-x/no-noninteractive-tabindex
              tabIndex={0}
              className="mnml-split-handle"
              data-dragging={resize.dragging || undefined}
              {...resize.handlers}
            />
          )}
          {bottom}
        </div>
      )}
    </div>
  );
}
