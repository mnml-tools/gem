/**
 * The connection log (plan §2.7, §3.5): a virtual list of mono lines (time, kind, text) from
 * the tab's ring buffer. It follows new lines while scrolled to the end and pauses when the
 * user scrolls up. Lines are selectable (click, Shift/Mod+click, Mod+A) for Copy.
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
} from 'react';
import { VirtualList, type VirtualListHandle } from '@/components/ui/VirtualList/VirtualList';
import { formatClock } from '@/lib/format';
import { logBuffer, useSessionData } from '@/state/sessionData';
import { panelStrings } from '@/strings/panel';

const t = panelStrings.log_;
const ROW_HEIGHT = 20;

export type LogSelection = ReadonlySet<number>;

type Props = {
  connId: string;
  selection: LogSelection;
  onSelectionChange: (selection: LogSelection) => void;
};

export function LogView({ connId, selection, onSelectionChange }: Props) {
  const version = useSessionData((state) => state.logVersion[connId] ?? 0);
  const buffer = logBuffer(connId);
  const count = buffer.size;
  const list = useRef<VirtualListHandle>(null);
  const [follow, setFollow] = useState(true);
  const anchor = useRef<number | null>(null);
  // Selections index the buffer; dropping lines from its front shifts them, so reset.
  const dropped = useRef(buffer.dropped);

  useEffect(() => {
    if (buffer.dropped !== dropped.current) {
      dropped.current = buffer.dropped;
      if (selection.size) onSelectionChange(new Set());
    }
  }, [version, buffer, selection, onSelectionChange]);

  useLayoutEffect(() => {
    if (follow) list.current?.scrollToBottom();
  }, [version, follow]);

  const select = useCallback(
    (index: number, shift: boolean, toggle: boolean) => {
      if (shift && anchor.current != null) {
        const [start, end] =
          anchor.current < index ? [anchor.current, index] : [index, anchor.current];
        onSelectionChange(new Set(Array.from({ length: end - start + 1 }, (_, i) => start + i)));
        return;
      }
      anchor.current = index;
      if (toggle) {
        const next = new Set(selection);
        if (next.has(index)) next.delete(index);
        else next.add(index);
        onSelectionChange(next);
      } else onSelectionChange(new Set([index]));
    },
    [selection, onSelectionChange],
  );

  const renderRow = (index: number, style: CSSProperties) => {
    const line = buffer.at(index);
    if (!line) return null;
    return (
      // Pointer selection only; the keyboard selects all with Mod+A and copies with Copy.
      // eslint-disable-next-line jsx-a11y-x/click-events-have-key-events
      <div
        // `seq` restarts with each connection attempt; the position in the stream does not.
        key={buffer.dropped + index}
        role="option"
        aria-selected={selection.has(index)}
        tabIndex={-1}
        className="gem-log-row"
        data-kind={line.kind}
        style={style}
        onClick={(event) => select(index, event.shiftKey, event.ctrlKey || event.metaKey)}
      >
        <span className="gem-log-time">{formatClock(line.ts)}</span>
        <span className="gem-log-kind">{t.kinds[line.kind]}</span>
        <span className="gem-log-text">{line.text}</span>
      </div>
    );
  };

  return (
    <div className="gem-log">
      <VirtualList
        ref={list}
        count={count}
        rowHeight={ROW_HEIGHT}
        overscan={12}
        renderRow={renderRow}
        className="gem-log-list"
        role="listbox"
        aria-label={t.label}
        aria-multiselectable
        tabIndex={0}
        onScroll={() => setFollow(list.current?.isAtBottom() ?? true)}
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
            event.preventDefault();
            onSelectionChange(new Set(Array.from({ length: count }, (_, i) => i)));
          } else if (event.key === 'End') {
            event.preventDefault();
            setFollow(true);
            list.current?.scrollToBottom();
          }
        }}
        empty={<div className="gem-list-message">{t.empty}</div>}
      />
      {!follow && count > 0 && (
        <button
          type="button"
          className="mnml-button gem-log-resume"
          onClick={() => {
            setFollow(true);
            list.current?.scrollToBottom();
          }}
        >
          {t.paused} · {t.resume}
        </button>
      )}
    </div>
  );
}
