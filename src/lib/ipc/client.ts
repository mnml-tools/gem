import { invoke } from '@tauri-apps/api/core';
import type { AppError } from './bindings/AppError';
import type { ErrorCode } from './bindings/ErrorCode';
import type { HostKeyInfo } from './bindings/HostKeyInfo';

/** Error raised by a failed command, carrying the stable code from Rust. */
export class IpcError extends Error {
  readonly code: ErrorCode;
  /** The server identity to show for `host_key_*` and `certificate_untrusted`. */
  readonly hostKey: HostKeyInfo | null;

  constructor(code: ErrorCode, message: string, hostKey: HostKeyInfo | null = null) {
    super(message);
    this.name = 'IpcError';
    this.code = code;
    this.hostKey = hostKey;
  }
}

function isAppError(value: unknown): value is AppError {
  return (
    typeof value === 'object' &&
    value !== null &&
    'code' in value &&
    'message' in value &&
    typeof (value as AppError).message === 'string'
  );
}

export function toIpcError(error: unknown): IpcError {
  if (error instanceof IpcError) return error;
  if (isAppError(error)) return new IpcError(error.code, error.message, error.hostKey ?? null);
  const message = error instanceof Error ? error.message : String(error);
  return new IpcError('internal', message);
}

/** Invokes a Rust command, normalising every failure to {@link IpcError}. */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toIpcError(error);
  }
}
