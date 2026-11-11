/**
 * A region that could not show its content: the kit's EmptyState (icon, title), then what went
 * wrong in an error `Message`, then the actions. (The kit's `notice` slot puts the message above
 * the icon, which reads upside down for errors.)
 */
import { EmptyState, Message } from '@mnml-tools/ui';
import type { ReactNode } from 'react';
import { sentence } from '@/lib/errors';

type Props = { icon: ReactNode; title: string; message?: string | null; children?: ReactNode };

export function RegionError({ icon, title, message, children }: Props) {
  return (
    <div className="gem-region-error">
      <EmptyState icon={icon} title={title} />
      {message && <Message tone="error">{sentence(message)}</Message>}
      {children && <div className="gem-empty-action">{children}</div>}
    </div>
  );
}
