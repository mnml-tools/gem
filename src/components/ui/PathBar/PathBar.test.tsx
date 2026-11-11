import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fitCrumbs } from './fitCrumbs';
import { PathBar } from './PathBar';

describe('fitCrumbs', () => {
  it('keeps everything when it fits', () => {
    expect(fitCrumbs([10, 40, 40], 100, 30)).toBe(0);
  });

  it('folds leading crumbs until the rest and the ellipsis fit', () => {
    // All: 150. Without the first: 30 + 140 = 170 (a 50px first crumb: 190 → 170 fits); two: 30 + 100 = 130; three: 30 + 60 = 90.
    expect(fitCrumbs([50, 40, 40, 60], 170, 30)).toBe(1);
    expect(fitCrumbs([10, 40, 40, 60], 140, 30)).toBe(2);
    expect(fitCrumbs([10, 40, 40, 60], 120, 30)).toBe(3);
  });

  it('always keeps the last crumb, even when it alone is too wide', () => {
    expect(fitCrumbs([10, 40, 300], 100, 30)).toBe(2);
    expect(fitCrumbs([500], 100, 30)).toBe(0);
  });
});

describe('PathBar', () => {
  afterEach(() => vi.restoreAllMocks());

  /**
   * Lays the bar out: 8px per character, the bar `width` wide. Like the stylesheet, the deepest
   * crumb shrinks (to 20px here) unless the bar is measuring its natural width.
   */
  function layout(width: number) {
    vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (
      this: HTMLElement,
    ) {
      const natural = (this.textContent ?? '').length * 8 + 10;
      const shrinks = this.matches('li[data-last]') && !this.closest('ol[data-measuring]');
      return shrinks ? Math.min(natural, 20) : natural;
    });
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (
      this: HTMLElement,
    ) {
      return this.classList.contains('mnml-pathbar') ? width : 0;
    });
  }

  const props = {
    label: 'Remote path',
    editLabel: 'Edit path',
    onNavigate: () => {},
  };

  it('shows every crumb when the path fits', () => {
    layout(800);
    render(<PathBar {...props} path="/home/gem/it" />);
    const nav = screen.getByRole('navigation', { name: 'Remote path' });
    expect(within(nav).queryByRole('button', { name: 'Edit path' })).toBeNull();
    expect(
      within(nav)
        .getAllByRole('button')
        .map((button) => button.textContent),
    ).toEqual(['/', 'home', 'gem', 'it']);
  });

  const visibleCrumbs = () =>
    within(screen.getByRole('navigation', { name: 'Remote path' }))
      .getAllByRole('button')
      .map((button) => button.textContent);

  it('measures the deepest crumb at its natural width, not shrunk', () => {
    // Natural: "/" 18, "home" 42, "/gem" 42, "/it" 34, "/e2e-commands" 114 = 250 > 194.
    layout(220);
    render(<PathBar {...props} path="/home/gem/it/e2e-commands" />);
    expect(visibleCrumbs()).toEqual(['…', 'it', 'e2e-commands']);
  });

  it('folds every leading crumb before the deepest one shrinks', () => {
    // Even "it" does not fit beside it: 30 + 34 + 114 = 178 > 154; 30 + 114 = 144 fits.
    layout(180);
    render(<PathBar {...props} path="/home/gem/it/e2e-commands" />);
    expect(visibleCrumbs()).toEqual(['…', 'e2e-commands']);
  });

  it('gives a deepest crumb too wide even alone a minimum of its natural width', () => {
    layout(120);
    render(<PathBar {...props} path="/home/gem/a-very-long-folder-name-indeed" />);
    expect(visibleCrumbs()).toEqual(['…', 'a-very-long-folder-name-indeed']);
    const last = screen.getByRole('button', { name: 'a-very-long-folder-name-indeed' });
    // The stylesheet keeps min(this, ~10ch) of it.
    expect(last.closest('li')!.style.getPropertyValue('--mnml-pathbar-last')).toBe('258px');
  });

  it('folds leading crumbs into "…" and keeps the deepest folder whole', async () => {
    layout(170);
    const user = userEvent.setup();
    render(<PathBar {...props} path="/home/gem/it/e2e-edit-target" />);
    const nav = screen.getByRole('navigation', { name: 'Remote path' });
    const visible = within(nav)
      .getAllByRole('button')
      .map((button) => button.textContent);
    expect(visible[0]).toBe('…');
    expect(visible.at(-1)).toBe('e2e-edit-target');
    expect(visible).not.toContain('home');
    // "…" turns the bar into the editable path.
    await user.click(within(nav).getByRole('button', { name: 'Edit path' }));
    expect(screen.getByRole('textbox', { name: 'Remote path' })).toHaveValue(
      '/home/gem/it/e2e-edit-target',
    );
  });
});
