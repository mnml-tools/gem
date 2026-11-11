/** Height state for a bottom pane: the height twin of the kit's width-only `useResize`. */
import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';

type Options = { storageKey: string; initial: number; minimum: number; maximum: number };

function readHeight(key: string): number | null {
  try {
    const saved = localStorage.getItem(key);
    if (saved !== null && saved.trim() !== '' && Number.isFinite(Number(saved)))
      return Number(saved);
  } catch {
    /* Storage unavailable: use the initial height. */
  }
  return null;
}

export function useHeightResize({ storageKey, initial, minimum, maximum }: Options) {
  const clamp = (value: number) => Math.round(Math.min(maximum, Math.max(minimum, value)));
  const [height, setHeight] = useState(() => clamp(readHeight(storageKey) ?? initial));
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ pointerId: number; y: number; height: number } | null>(null);

  const resize = (value: number) => {
    const next = clamp(value);
    setHeight(next);
    try {
      localStorage.setItem(storageKey, String(next));
    } catch {
      /* Keep the height for this session only. */
    }
  };

  const finish = () => {
    drag.current = null;
    setDragging(false);
    document.documentElement.classList.remove('mnml-resizing-rows');
  };

  useEffect(() => () => document.documentElement.classList.remove('mnml-resizing-rows'), []);

  return {
    height: clamp(height),
    dragging,
    minimum,
    maximum,
    handlers: {
      onPointerDown: (event: PointerEvent<HTMLDivElement>) => {
        if (event.button !== 0 || !event.isPrimary) return;
        event.preventDefault();
        event.currentTarget.focus();
        drag.current = { pointerId: event.pointerId, y: event.clientY, height };
        event.currentTarget.setPointerCapture(event.pointerId);
        setDragging(true);
        document.documentElement.classList.add('mnml-resizing-rows');
      },
      onPointerMove: (event: PointerEvent<HTMLDivElement>) => {
        const start = drag.current;
        // Dragging up makes the bottom pane taller.
        if (start?.pointerId === event.pointerId) resize(start.height + start.y - event.clientY);
      },
      onPointerUp: (event: PointerEvent<HTMLDivElement>) => {
        if (drag.current?.pointerId !== event.pointerId) return;
        finish();
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
      },
      onPointerCancel: finish,
      onLostPointerCapture: finish,
      onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
        const step = event.shiftKey ? 50 : 10;
        let next = height;
        if (event.key === 'ArrowUp') next += step;
        else if (event.key === 'ArrowDown') next -= step;
        else if (event.key === 'Home') next = minimum;
        else if (event.key === 'End') next = maximum;
        else return;
        event.preventDefault();
        resize(next);
      },
    },
  };
}
