/** TanStack Query keys: one place, so mutations invalidate exactly what they change. */
export const queryKeys = {
  appStatus: ['appStatus'] as const,
  settings: ['settings'] as const,
  sites: ['sites'] as const,
  /** Every listing of a session (invalidate after a mutation or a finished upload). */
  listings: (sessionId: string) => ['listing', sessionId] as const,
  listing: (sessionId: string, path: string) => ['listing', sessionId, path] as const,
  /** Every snippet list (invalidate after any snippet change). */
  allSnippets: ['snippets'] as const,
  /** Global snippets plus those of `siteId`. */
  snippets: (siteId: string | null) => ['snippets', siteId ?? ''] as const,
};
