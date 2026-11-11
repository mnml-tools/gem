/**
 * Toasts (`mnml-toast`), after Piw's: bottom centre, one at a time (the newest replaces the
 * previous), the kit's menu surface, paused while hovered or focused. Errors lead with an
 * icon and are announced as alerts; everything else is a polite status.
 */
import { CircleAlert, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { sentence } from '@/lib/errors';
import { ToastContext, type ToastOptions } from './context';
import './Toast.css';

type Current = ToastOptions & { id: number };

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toast, setToast] = useState<Current | null>(null);
  const counter = useRef(0);
  const show = useCallback((options: ToastOptions) => {
    counter.current += 1;
    setToast({ ...options, id: counter.current });
  }, []);
  const dismiss = useCallback(() => setToast(null), []);
  const api = useMemo(() => ({ show, dismiss }), [show, dismiss]);
  return (
    <ToastContext.Provider value={api}>
      {children}
      {createPortal(
        <div className="mnml-toast-region">
          {toast && <ToastView key={toast.id} toast={toast} onDismiss={dismiss} />}
        </div>,
        document.body,
      )}
    </ToastContext.Provider>
  );
}

function ToastView({ toast, onDismiss }: { toast: Current; onDismiss: () => void }) {
  const [paused, setPaused] = useState(false);
  const error = toast.tone === 'error';
  const duration = toast.duration ?? (error || toast.action ? 8000 : 4000);

  useEffect(() => {
    if (paused) return;
    const timer = window.setTimeout(onDismiss, duration);
    return () => window.clearTimeout(timer);
  }, [paused, duration, onDismiss]);

  return (
    <div
      className="mnml-toast"
      data-tone={error ? 'error' : undefined}
      role={error ? 'alert' : 'status'}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
    >
      {error && <CircleAlert size={14} aria-hidden className="mnml-toast-icon" />}
      <span className="mnml-toast-message">{sentence(toast.message)}</span>
      {toast.action && (
        <button
          type="button"
          className="mnml-button mnml-toast-action"
          onClick={() => {
            toast.action?.run();
            onDismiss();
          }}
        >
          {toast.action.label}
        </button>
      )}
      <button
        type="button"
        className="mnml-button mnml-icon-button mnml-toast-close"
        aria-label="Dismiss"
        onClick={onDismiss}
      >
        <X size={13} aria-hidden />
      </button>
    </div>
  );
}
