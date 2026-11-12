export const settingsStrings = {
  title: 'Settings',
  downloads: 'Download folder',
  downloadsDefault: 'Downloads (system default)',
  choose: 'Choose…',
  reset: 'Use default',
  open: 'Open',
  preview: 'First window of a file',
  previewHint: 'Larger files load the rest on demand.',
  confirmAbove: 'Ask before opening files over (MB)',
  concurrency: 'Parallel transfers per tab',
  keepalive: 'Keepalive (seconds, 0 to turn off)',
  showHidden: 'Show hidden files in new tabs',
  conflictPolicy: 'When a transfer target exists',
  policies: { ask: 'Ask', overwrite: 'Overwrite', skip: 'Skip', rename: 'Keep both' },
  appearanceHint: 'Colour mode and accent are in the palette button of the title bar.',
  save: 'Save settings',
  saved: 'Settings saved',
  invalid: 'Check the highlighted values',
} as const;

export const updateStrings = {
  section: 'Updates',
  checkAtStartup: 'Check for updates when Gem starts',
  checkNow: 'Check for updates',
  checking: 'Checking…',
  version: (version: string) => `Gem ${version}`,
  upToDate: 'Gem is up to date.',
  available: (version: string) => `Gem ${version} is available.`,
  failed: (message: string) => `Could not check for updates: ${message}`,
  desktopOnly: 'Updates are checked in the desktop app.',
  busy: 'Wait for the transfers to finish, or cancel them, before updating.',
  discard:
    'Unsaved files will be lost. Cancel to save them first, or discard the changes and update.',
} as const;

export const helpStrings = {
  title: 'Keyboard shortcuts',
} as const;
