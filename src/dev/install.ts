/**
 * Dev-only: serves the UI from an in-memory mock backend when it runs in a plain browser
 * (`pnpm dev`, then http://localhost:1426). `main.tsx` imports this module only when
 * `import.meta.env.DEV` is true and the page is not inside Tauri, so production builds never
 * contain it. Usage: docs/ui-design-system.md#browser-preview.
 *
 * - Commands: `mockIPC` routes every `invoke` to `mock-backend.ts`.
 * - Channels: `connect` receives a Tauri `Channel`; messages reach it the way Rust's do, as
 *   `{ index, message }` through the callback that `transformCallback` registered.
 * - Events: the event plugin (`listen`, `unlisten`) is emulated, so `gem://drag` and
 *   `gem://drop` can be fired from the console (`__GEM_MOCK__.dropFiles([...], x, y)`).
 */
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { createMockBackend, type MockControls, type MockOptions } from './mock-backend';

type Internals = {
  callbacks?: Map<number, (data: unknown) => void>;
  runCallback?: (id: number, data: unknown) => void;
};

type MockGlobals = {
  __TAURI_INTERNALS__?: Internals;
  __GEM_MOCK__?: MockControls;
};

const globals = window as unknown as MockGlobals;
const PREFIX = '[gem dev mock]';

/** Installs the mock IPC; returns the server-side controls (tests use them directly). */
export function installMock(
  options: Partial<Pick<MockOptions, 'latency' | 'mode' | 'log' | 'keyring'>> = {},
) {
  const listeners = new Map<number, string>();
  const deliver = (event: string, payload: unknown) => {
    const callbacks = globals.__TAURI_INTERNALS__?.callbacks;
    for (const [id, name] of listeners) {
      if (name === event) callbacks?.get(id)?.({ event, id, payload });
    }
  };
  const channelIndex = new Map<number, number>();

  const backend = createMockBackend({
    ...options,
    openChannel: (channel) => {
      const id = (channel as { id?: number } | null)?.id;
      return (message) => {
        if (id == null) return;
        const index = channelIndex.get(id) ?? 0;
        channelIndex.set(id, index + 1);
        // Like Rust, channel messages arrive after the command that produced them.
        setTimeout(() => globals.__TAURI_INTERNALS__?.callbacks?.get(id)?.({ index, message }), 0);
      };
    },
    emit: (event, payload) => setTimeout(() => deliver(event, payload), 0),
  });

  mockIPC((command, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    switch (command) {
      case 'plugin:event|listen': {
        const id = Number(args['handler']);
        listeners.set(id, String(args['event']));
        return id;
      }
      case 'plugin:event|unlisten':
        listeners.delete(Number(args['eventId']));
        return null;
      case 'plugin:event|emit':
        deliver(String(args['event']), args['payload']);
        return null;
      default:
        return backend.invoke(command, args);
    }
  });
  return backend.controls;
}

export function uninstallMock(): void {
  clearMocks();
}

export function installMockBackend(): void {
  const params = new URLSearchParams(window.location.search);
  const latency = params.has('latency') ? Number(params.get('latency')) : undefined;
  const mode = params.get('mock') === 'empty' ? 'empty' : 'demo';
  const keyring = params.get('keyring') !== '0';
  // The mock announces itself so nobody mistakes demo data for a real server.
  // eslint-disable-next-line no-console
  const log = (message: string) => console.info(`${PREFIX} ${message}`);
  const controls = installMock({ latency, mode, log, keyring });
  globals.__GEM_MOCK__ = controls;
  log(
    `${mode === 'empty' ? 'no saved sites' : 'demo sites and a fake remote tree'} in memory ` +
      '(dev only, resets on reload). prod-api asks to trust its host key, staging asks for a ' +
      'password ("wrong" fails), backup-box has a changed host key. Console: ' +
      "__GEM_MOCK__.changeOnServer('/var/www/api/config.yaml'), __GEM_MOCK__.dropConnection(), " +
      "__GEM_MOCK__.dropFiles(['a.txt'], x, y). URL: ?mock=empty, ?latency=0, ?keyring=0.",
  );
}
