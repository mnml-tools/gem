/**
 * User-facing strings, one file per area, so a future i18n layer can replace these objects
 * without touching components (plan §0.1.14). English, sentence case, verb-first buttons, no
 * "OK"/"Yes"/"Submit", no exclamation marks. Objects are named in curly quotes.
 */
export const common = {
  appName: 'gem',
  appNote: 'file manager',
  cancel: 'Cancel',
  close: 'Close',
  save: 'Save',
  retry: 'Retry',
  delete: 'Delete',
  done: 'Done',
  dismiss: 'Dismiss',
  insecure: 'Insecure',
  insecureHint: 'Plain FTP: the password and files travel unencrypted',
  loading: 'Loading…',
  copiedPath: 'Copied path',
  copyFailed: 'Couldn’t copy to the clipboard',
  genericError: 'Something went wrong',
  keyringUnavailable:
    'The system keyring isn’t available; Gem will ask for the password when connecting.',
} as const;

/** `“name”` */
export const quoted = (name: string) => `“${name}”`;
