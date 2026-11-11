import { lazy, Suspense } from 'react';
import { AppProviders } from './providers';

const WorkspaceApp = lazy(() => import('@/views/workspace/WorkspaceApp'));

export function Root() {
  return (
    <AppProviders>
      <Suspense fallback={null}>
        <WorkspaceApp />
      </Suspense>
    </AppProviders>
  );
}
