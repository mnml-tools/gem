/**
 * A fixed-row-height virtual list: only the rows in view (plus `overscan`) are in the DOM, so
 * 10,000 entries scroll as fluidly as 10. Rows are absolutely positioned inside a spacer of
 * the full height. Designed for the kit (`mnml-virtual-list`): no colours of its own.
 */
import {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type ReactNode,
  type UIEvent,
} from 'react';
import './VirtualList.css';

export type VirtualListHandle = {
  element: HTMLDivElement | null;
  /** Scrolls the least amount needed to show row `index`. */
  scrollToIndex: (index: number) => void;
  scrollToBottom: () => void;
  isAtBottom: () => boolean;
};

type Props = Omit<HTMLAttributes<HTMLDivElement>, 'children' | 'onScroll'> & {
  count: number;
  rowHeight: number;
  renderRow: (index: number, style: CSSProperties) => ReactNode;
  overscan?: number;
  /** Rendered over the rows' area when `count` is 0. */
  empty?: ReactNode;
  onScroll?: (event: UIEvent<HTMLDivElement>) => void;
};

/** Height assumed before the first measurement (and in environments without layout). */
const FALLBACK_HEIGHT = 720;

export const VirtualList = forwardRef<VirtualListHandle, Props>(function VirtualList(
  { count, rowHeight, renderRow, overscan = 8, empty, className, onScroll, ...rest },
  ref,
) {
  const element = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);

  useLayoutEffect(() => {
    const node = element.current;
    if (!node) return;
    const measure = () => setHeight(node.clientHeight);
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  // Keep the scroll position valid when the list shrinks (e.g. a filter).
  useLayoutEffect(() => {
    const node = element.current;
    if (!node) return;
    const max = Math.max(0, count * rowHeight - node.clientHeight);
    if (node.scrollTop > max) {
      node.scrollTop = max;
      setScrollTop(max);
    }
  }, [count, rowHeight]);

  const scrollToIndex = useCallback(
    (index: number) => {
      const node = element.current;
      if (!node || index < 0) return;
      const top = index * rowHeight;
      const bottom = top + rowHeight;
      const viewport = node.clientHeight || FALLBACK_HEIGHT;
      if (top < node.scrollTop) node.scrollTop = top;
      else if (bottom > node.scrollTop + viewport) node.scrollTop = bottom - viewport;
      setScrollTop(node.scrollTop);
    },
    [rowHeight],
  );

  useImperativeHandle(
    ref,
    () => ({
      get element() {
        return element.current;
      },
      scrollToIndex,
      scrollToBottom: () => {
        const node = element.current;
        if (!node) return;
        node.scrollTop = node.scrollHeight;
        setScrollTop(node.scrollTop);
      },
      isAtBottom: () => {
        const node = element.current;
        if (!node) return true;
        return node.scrollTop + node.clientHeight >= node.scrollHeight - rowHeight / 2;
      },
    }),
    [scrollToIndex, rowHeight],
  );

  const viewport = height || FALLBACK_HEIGHT;
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const last = Math.min(count, Math.ceil((scrollTop + viewport) / rowHeight) + overscan);
  const rows: ReactNode[] = [];
  for (let index = first; index < last; index += 1) {
    rows.push(
      renderRow(index, {
        position: 'absolute',
        top: index * rowHeight,
        left: 0,
        right: 0,
        height: rowHeight,
      }),
    );
  }

  return (
    <div
      {...rest}
      ref={element}
      className={className ? `mnml-virtual-list ${className}` : 'mnml-virtual-list'}
      onScroll={(event) => {
        setScrollTop(event.currentTarget.scrollTop);
        onScroll?.(event);
      }}
    >
      <div className="mnml-virtual-list-spacer" style={{ height: count * rowHeight }}>
        {rows}
      </div>
      {count === 0 && empty}
    </div>
  );
});
