export const workspaceStrings = {
  window: {
    controls: 'Window controls',
    minimize: 'Minimize window',
    maximize: 'Maximize window',
    restore: 'Restore window',
    close: 'Close window',
  },
  status: {
    connected: 'Connected',
    connecting: 'Connecting…',
    disconnected: 'Disconnected',
    failed: 'Not connected',
    idle: 'No connection',
  },
  tabs: {
    label: 'Connections',
    sites: 'Sites',
    newTab: 'New tab',
    close: (name: string) => `Close ${name}`,
    menu: (name: string) => `Actions for ${name}`,
    reconnect: 'Reconnect',
    duplicate: 'Open in new tab',
    closeTab: 'Close tab',
    closeOthers: 'Close other tabs',
  },
  settings: 'Settings',
  shortcuts: 'Keyboard shortcuts',
  statusBar: {
    items: (count: number) => `${count.toLocaleString('en-US')} ${count === 1 ? 'item' : 'items'}`,
    idle: 'idle',
  },
  closeConnection: {
    title: 'Close connection?',
    dirty: (name: string, files: number) =>
      `${files === 1 ? 'A file has' : `${files} files have`} unsaved changes in ${name}. Closing the tab discards them.`,
    transfers: (count: number) =>
      `${count === 1 ? 'A transfer is' : `${count} transfers are`} still running and will be cancelled.`,
    confirm: 'Close tab',
  },
  closeWindow: {
    title: 'Quit Gem?',
    dirty: (files: number) =>
      `${files === 1 ? 'A file has' : `${files} files have`} unsaved changes. Quitting discards them.`,
    transfers: (count: number) =>
      `${count === 1 ? 'A transfer is' : `${count} transfers are`} still running and will be cancelled.`,
    confirm: 'Quit',
  },
  dataError: 'Gem couldn’t open its data; sites can’t be saved.',
} as const;
