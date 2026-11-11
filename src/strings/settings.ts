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

export const helpStrings = {
  title: 'Keyboard shortcuts',
} as const;
