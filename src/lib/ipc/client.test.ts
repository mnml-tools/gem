import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, describe, expect, it } from 'vitest';
import { call, IpcError, toIpcError } from './client';

afterEach(() => clearMocks());

describe('call', () => {
  it('returns the command result', async () => {
    mockIPC((cmd, args) => (cmd === 'echo' ? args : null));
    await expect(call('echo', { value: 1 })).resolves.toEqual({ value: 1 });
  });

  it('maps Rust errors to IpcError with their code', async () => {
    mockIPC(() => {
      throw { code: 'conflict', message: 'the file changed on the server', hostKey: null };
    });
    const error = await call('save').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(IpcError);
    expect(error).toMatchObject({ code: 'conflict', message: 'the file changed on the server' });
  });

  it('keeps the host key of trust errors', async () => {
    const hostKey = {
      host: 'example.test',
      port: 22,
      keyType: 'ssh-ed25519',
      fingerprint: 'SHA256:abc',
      previousFingerprint: null,
    };
    mockIPC(() => {
      throw { code: 'host_key_unknown', message: 'not trusted', hostKey };
    });
    const error = await call('connect').catch((e: unknown) => e);
    expect(error).toMatchObject({ code: 'host_key_unknown', hostKey });
  });
});

describe('toIpcError', () => {
  it('wraps unknown failures as internal errors', () => {
    expect(toIpcError('boom')).toMatchObject({ code: 'internal', message: 'boom' });
    expect(toIpcError(new Error('bad'))).toMatchObject({ code: 'internal', message: 'bad' });
  });
});
