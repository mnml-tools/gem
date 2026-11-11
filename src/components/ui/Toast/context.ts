import { createContext, useContext } from 'react';

export type ToastOptions = {
  message: string;
  tone?: 'info' | 'error';
  action?: { label: string; run: () => void };
  /** Milliseconds; 4s for info, 8s for errors and toasts with an action. */
  duration?: number;
};

export type ToastApi = { show: (options: ToastOptions) => void; dismiss: () => void };

export const ToastContext = createContext<ToastApi | null>(null);

const noop: ToastApi = { show: () => {}, dismiss: () => {} };

/** `toast.show({ message: 'Saved' })`. Outside a provider it does nothing. */
export function useToast(): ToastApi {
  return useContext(ToastContext) ?? noop;
}
