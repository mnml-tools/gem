/**
 * Breadcrumbs that become an editable path (`mnml-pathbar`): each segment navigates; a click on
 * the empty end of the bar, the pencil, or `edit()` (Mod+L) turns it into a mono text field.
 * Enter goes there, Esc or blur cancels. Mono, like every path in the family. When the path is
 * wider than the bar, leading crumbs fold into "…" (which edits the path), all of them before
 * the deepest crumb gives up any width; only then does it shrink, never below ~10ch, with an
 * ellipsis at the end.
 */
import { Pencil } from 'lucide-react';
import {
  forwardRef,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from 'react';
import { breadcrumbs, normalizePath } from '@/lib/paths';
import { fitCrumbs } from './fitCrumbs';
import './PathBar.css';

export type PathBarHandle = { edit: () => void };

/** The pencil's minimum width, kept free at the end of the bar. */
const FILL_MINIMUM = 26;
/** The "…" button and its separator. */
const ELLIPSIS_WIDTH = 30;

type Props = {
  path: string;
  onNavigate: (path: string) => void;
  /** Accessible name of the bar, e.g. "Remote path". */
  label: string;
  editLabel: string;
  disabled?: boolean;
  className?: string;
};

export const PathBar = forwardRef<PathBarHandle, Props>(function PathBar(
  { path, onNavigate, label, editLabel, disabled = false, className },
  ref,
) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(path);
  const input = useRef<HTMLInputElement>(null);
  const crumbsList = useRef<HTMLOListElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  /** Leading crumbs folded into "…"; `null` while the next layout measures them all. */
  const [hidden, setHidden] = useState<number | null>(null);
  /** The deepest crumb's natural width: its minimum is the smaller of that and ~10ch. */
  const [lastWidth, setLastWidth] = useState<number | null>(null);
  const [width, setWidth] = useState(0);

  const startEditing = () => {
    if (disabled) return;
    setDraft(path);
    setEditing(true);
  };

  useImperativeHandle(ref, () => ({ edit: startEditing }));

  useLayoutEffect(() => {
    if (editing) {
      input.current?.focus();
      input.current?.select();
    }
  }, [editing]);

  // Follow the bar's width; a new width or path folds the crumbs again.
  useLayoutEffect(() => {
    const node = bar.current;
    if (!node) return;
    const measure = () => setWidth(node.clientWidth);
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useLayoutEffect(() => setHidden(null), [path, width, editing]);

  // With every crumb rendered at its natural width (`data-measuring` stops the deepest one from
  // shrinking, which would make it look like it fits), measure them and fold the leading ones
  // that do not fit, before the browser paints.
  useLayoutEffect(() => {
    if (hidden !== null || editing) return;
    const items = Array.from(crumbsList.current?.children ?? []) as HTMLElement[];
    const widths = items.map((item) => item.offsetWidth);
    setLastWidth(widths.at(-1) ?? null);
    setHidden(fitCrumbs(widths, Math.max(0, width - FILL_MINIMUM), ELLIPSIS_WIDTH));
  }, [hidden, editing, width]);

  const commit = () => {
    setEditing(false);
    const target = draft.trim();
    if (target && normalizePath(target) !== normalizePath(path)) {
      onNavigate(normalizePath(target.startsWith('/') ? target : `${path}/${target}`));
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      commit();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      setEditing(false);
    }
  };

  const crumbs = breadcrumbs(path);
  const folded = hidden ?? 0;

  return (
    <div ref={bar} className={className ? `mnml-pathbar ${className}` : 'mnml-pathbar'}>
      {editing ? (
        <input
          ref={input}
          className="mnml-pathbar-input"
          aria-label={label}
          value={draft}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
          onBlur={() => setEditing(false)}
        />
      ) : (
        <>
          <nav aria-label={label} className="mnml-pathbar-nav">
            {folded > 0 && (
              <button
                type="button"
                className="mnml-button mnml-pathbar-more"
                aria-label={editLabel}
                title={crumbs[folded - 1]!.path}
                disabled={disabled}
                onClick={startEditing}
              >
                …
              </button>
            )}
            <ol ref={crumbsList} data-measuring={hidden === null || undefined}>
              {crumbs.map((crumb, index) => {
                const last = index === crumbs.length - 1;
                return (
                  <li
                    key={crumb.path}
                    hidden={index < folded}
                    data-last={last || undefined}
                    style={
                      last && lastWidth !== null
                        ? ({ '--mnml-pathbar-last': `${lastWidth}px` } as CSSProperties)
                        : undefined
                    }
                  >
                    {(index >= 2 || (folded > 0 && index === folded)) && (
                      <span className="mnml-pathbar-sep" aria-hidden>
                        /
                      </span>
                    )}
                    <button
                      type="button"
                      className="mnml-button"
                      aria-current={last ? 'location' : undefined}
                      disabled={disabled}
                      title={crumb.path}
                      onClick={() => (last ? startEditing() : onNavigate(crumb.path))}
                    >
                      {crumb.name}
                    </button>
                  </li>
                );
              })}
            </ol>
          </nav>
          <button
            type="button"
            className="mnml-button mnml-pathbar-fill"
            aria-label={editLabel}
            title={editLabel}
            disabled={disabled}
            onClick={startEditing}
          >
            <Pencil size={12} aria-hidden />
          </button>
        </>
      )}
    </div>
  );
});
