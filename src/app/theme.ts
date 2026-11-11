import { createTheme } from '@mnml-tools/ui';
import { syncNativeTheme } from '@mnml-tools/ui/tauri';

/**
 * Colour mode and accent, kept by the kit's theme store (`gem.theme`, `gem.accent` in local
 * storage). Created at module load so the first paint has the right theme.
 */
export const theme = createTheme({ storagePrefix: 'gem', onThemeChange: syncNativeTheme });

export type Platform = 'macos' | 'windows' | 'linux';

export function detectPlatform(userAgent: string = navigator.userAgent): Platform {
  if (/Mac|iPhone|iPad/.test(userAgent)) return 'macos';
  if (/Windows/.test(userAgent)) return 'windows';
  return 'linux';
}
