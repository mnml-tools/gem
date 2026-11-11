/** Keeps cached listings in step with what the viewer learns about a file. */
import type { QueryClient } from '@tanstack/react-query';
import type { Entry } from '@/lib/ipc/bindings/Entry';
import type { Listing } from '@/lib/ipc/bindings/Listing';
import { parentPath } from '@/lib/paths';
import { queryKeys } from './keys';

/**
 * Updates a file's row in its folder's cached listing (size, mtime) after an open, a reload or
 * a save, so the list never shows a stale size next to the editor. Other rows are untouched.
 */
export function patchListingEntry(
  queryClient: QueryClient,
  sessionId: string,
  path: string,
  patch: Partial<Pick<Entry, 'size' | 'mtime'>>,
): void {
  queryClient.setQueryData<Listing>(queryKeys.listing(sessionId, parentPath(path)), (listing) =>
    listing
      ? {
          ...listing,
          entries: listing.entries.map((entry) =>
            entry.path === path && entry.kind === 'file' ? { ...entry, ...patch } : entry,
          ),
        }
      : listing,
  );
}
