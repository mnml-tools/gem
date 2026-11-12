/**
 * Renders the whole window against the dev mock backend (src/dev), with no latency: the
 * critical flows run through the real stores, the real IPC wrappers and `mockIPC`.
 */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { clearMocks } from '@tauri-apps/api/mocks';
import { cleanup, render } from '@testing-library/react';
import { installMock } from '@/dev/install';
import { useFileTabs } from '@/state/fileTabs';
import { useSessionData } from '@/state/sessionData';
import { useUi } from '@/state/ui';
import { useUpdates } from '@/state/updates';
import { useWorkspace } from '@/state/workspace';
import WorkspaceApp from '@/views/workspace/WorkspaceApp';

export function resetStores(): void {
  try {
    localStorage.clear();
  } catch {
    /* jsdom always has storage */
  }
  useWorkspace.setState({ tabs: [], activeId: null });
  useFileTabs.setState({ byConn: {} });
  useUi.setState({ settingsOpen: false, helpOpen: false, siteDialog: null });
  useUpdates.setState({ update: null, checking: false, message: null, error: null });
  useSessionData.setState({
    panelOpen: true,
    panelView: 'log',
    logVersion: {},
    unseenError: {},
    transfers: {},
    speeds: {},
  });
}

export function renderApp(options: { mode?: 'demo' | 'empty'; keyring?: boolean } = {}) {
  resetStores();
  const controls = installMock({
    latency: 0,
    mode: options.mode ?? 'demo',
    keyring: options.keyring ?? true,
  });
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Infinity, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  });
  const result = render(
    <QueryClientProvider client={client}>
      <WorkspaceApp />
    </QueryClientProvider>,
  );
  return { ...result, controls, client };
}

export function cleanupApp(): void {
  // Unmount first: effects unlisten through the mocked IPC.
  cleanup();
  clearMocks();
}
