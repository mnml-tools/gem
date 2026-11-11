/**
 * Connection tabs (plan §3.1): one per live session, plus Sites tabs (the Sites page opened
 * with Mod+T). Tabs stay mounted while inactive, so this store only keeps what other parts of
 * the window need: identity, state, current path and the session returned by `connect`.
 */
import { create } from 'zustand';
import type { AuthMethod } from '@/lib/ipc/bindings/AuthMethod';
import type { ConnectTarget } from '@/lib/ipc/bindings/ConnectTarget';
import type { ErrorCode } from '@/lib/ipc/bindings/ErrorCode';
import type { Protocol } from '@/lib/ipc/bindings/Protocol';
import type { SessionInfo } from '@/lib/ipc/bindings/SessionInfo';
import type { SiteColor } from '@/lib/ipc/bindings/SiteColor';

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'failed';

export type SitesTab = { id: string; kind: 'sites' };

export type ConnectionTab = {
  id: string;
  kind: 'connection';
  target: ConnectTarget;
  label: string;
  protocol: Protocol;
  insecure: boolean;
  color: SiteColor | null;
  siteId: string | null;
  /** For the password prompt: who connects where, and with what. */
  user: string;
  host: string;
  port: number;
  auth: AuthMethod;
  status: ConnectionStatus;
  /** Why the session dropped or failed, as shown in the tab. */
  error: { code: ErrorCode; message: string } | null;
  session: SessionInfo | null;
  /** Current remote directory; `null` until connected. */
  path: string | null;
  /** Bumped to start a new connection attempt (Retry). */
  attempt: number;
  /** Entries shown by the file list, for the status bar. */
  items: number | null;
};

export type WorkspaceTab = SitesTab | ConnectionTab;

export type ConnectionMeta = Pick<
  ConnectionTab,
  | 'target'
  | 'label'
  | 'protocol'
  | 'insecure'
  | 'color'
  | 'siteId'
  | 'user'
  | 'host'
  | 'port'
  | 'auth'
>;

type WorkspaceState = {
  tabs: WorkspaceTab[];
  activeId: string | null;
  openSitesTab: () => string;
  /** Opens a connection; `replaceId` turns that tab (a Sites tab) into the connection. */
  openConnection: (meta: ConnectionMeta, replaceId?: string | null) => string;
  selectTab: (id: string) => void;
  /** Moves the selection by `delta` tabs, wrapping around. */
  cycleTabs: (delta: number) => void;
  removeTab: (id: string) => void;
  updateConnection: (id: string, patch: Partial<ConnectionTab>) => void;
  retry: (id: string, meta?: Partial<ConnectionMeta>) => void;
};

let counter = 0;
const newTabId = (prefix: string) => `${prefix}-${Date.now().toString(36)}-${(counter += 1)}`;

export const useWorkspace = create<WorkspaceState>()((set, get) => ({
  tabs: [],
  activeId: null,

  openSitesTab: () => {
    const id = newTabId('sites');
    set((state) => ({ tabs: [...state.tabs, { id, kind: 'sites' }], activeId: id }));
    return id;
  },

  openConnection: (meta, replaceId) => {
    const id = newTabId('conn');
    const tab: ConnectionTab = {
      id,
      kind: 'connection',
      ...meta,
      status: 'connecting',
      error: null,
      session: null,
      path: null,
      attempt: 1,
      items: null,
    };
    set((state) => {
      const index = replaceId ? state.tabs.findIndex((item) => item.id === replaceId) : -1;
      const tabs = [...state.tabs];
      if (index >= 0) tabs.splice(index, 1, tab);
      else tabs.push(tab);
      return { tabs, activeId: id };
    });
    return id;
  },

  selectTab: (id) => {
    if (get().tabs.some((tab) => tab.id === id)) set({ activeId: id });
  },

  cycleTabs: (delta) => {
    const { tabs, activeId } = get();
    if (tabs.length < 2) return;
    const index = Math.max(
      0,
      tabs.findIndex((tab) => tab.id === activeId),
    );
    const next = tabs[(index + delta + tabs.length) % tabs.length];
    if (next) set({ activeId: next.id });
  },

  removeTab: (id) =>
    set((state) => {
      const index = state.tabs.findIndex((tab) => tab.id === id);
      if (index < 0) return state;
      const tabs = state.tabs.filter((tab) => tab.id !== id);
      let activeId = state.activeId;
      if (activeId === id) activeId = (tabs[index] ?? tabs[index - 1])?.id ?? null;
      return { tabs, activeId };
    }),

  updateConnection: (id, patch) =>
    set((state) => ({
      tabs: state.tabs.map((tab) =>
        tab.id === id && tab.kind === 'connection' ? { ...tab, ...patch } : tab,
      ),
    })),

  retry: (id, meta) =>
    set((state) => ({
      tabs: state.tabs.map((tab) =>
        tab.id === id && tab.kind === 'connection'
          ? {
              ...tab,
              ...meta,
              status: 'connecting',
              error: null,
              session: null,
              attempt: tab.attempt + 1,
            }
          : tab,
      ),
    })),
}));

export function getConnectionTab(id: string): ConnectionTab | null {
  const tab = useWorkspace.getState().tabs.find((item) => item.id === id);
  return tab?.kind === 'connection' ? tab : null;
}

/** The active tab when it is a connection. */
export function useActiveConnection(): ConnectionTab | null {
  return useWorkspace((state) => {
    const tab = state.tabs.find((item) => item.id === state.activeId);
    return tab?.kind === 'connection' ? tab : null;
  });
}

/**
 * Labels for the tab strip and title bar: unique among the open connection tabs. Tabs that
 * share a label get `:port` (when their ports differ) and the protocol (when that differs),
 * then a counter for what is still the same (`gem@127.0.0.1`, `gem@127.0.0.1 (2)`).
 */
export function tabLabels(tabs: readonly WorkspaceTab[]): Map<string, string> {
  const connections = tabs.filter((tab): tab is ConnectionTab => tab.kind === 'connection');
  const groups = new Map<string, ConnectionTab[]>();
  for (const tab of connections) groups.set(tab.label, [...(groups.get(tab.label) ?? []), tab]);
  const labels = new Map<string, string>();
  for (const [label, group] of groups) {
    if (group.length === 1) {
      labels.set(group[0]!.id, label);
      continue;
    }
    const ports = new Set(group.map((tab) => tab.port)).size > 1;
    const protocols = new Set(group.map((tab) => tab.protocol)).size > 1;
    const seen = new Map<string, number>();
    for (const tab of group) {
      let text = label;
      if (ports && !label.endsWith(`:${tab.port}`)) text += `:${tab.port}`;
      if (protocols) text += ` · ${tab.protocol.toUpperCase()}`;
      const count = (seen.get(text) ?? 0) + 1;
      seen.set(text, count);
      labels.set(tab.id, count === 1 ? text : `${text} (${count})`);
    }
  }
  return labels;
}

/** The unique label of one tab (see `tabLabels`). */
export function useTabLabel(tab: ConnectionTab | null): string {
  return useWorkspace((state) => (tab ? (tabLabels(state.tabs).get(tab.id) ?? tab.label) : ''));
}
