import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Harness from './CodeHarness.svelte';

const code = 'echo one\necho two\necho three\necho four';

const lines = async (container: HTMLElement) => {
  await vi.waitFor(() =>
    expect(container.querySelector('pre.shiki')).not.toBeNull()
  );
  return [...container.querySelectorAll('pre .line')];
};

describe('Code.Root', () => {
  it('highlights the code with line numbers by default', async () => {
    const { container } = render(Harness, { code, lang: 'bash' });
    const ls = await lines(container);
    expect(ls).toHaveLength(4);
    expect(container.querySelector('pre')).toHaveClass('line-numbers');
    expect(container.querySelector('pre')?.getAttribute('style')).toBe('');
  });

  it('omits line numbers when hideLines is set', async () => {
    const { container } = render(Harness, { code, hideLines: true });
    await lines(container);
    expect(container.querySelector('pre')).not.toHaveClass('line-numbers');
  });

  it('marks single lines and inclusive ranges as highlighted', async () => {
    const { container } = render(Harness, {
      code,
      highlight: [1, [3, 4]],
      lang: 'bash'
    });
    const ls = await lines(container);
    expect(ls.map((l) => l.classList.contains('line--highlighted'))).toEqual([
      true,
      false,
      true,
      true
    ]);
  });

  it('highlights nothing without a highlight list', async () => {
    const { container } = render(Harness, { code, highlight: undefined });
    const ls = await lines(container);
    expect(ls.some((l) => l.classList.contains('line--highlighted'))).toBe(
      false
    );
  });

  it('copies the raw code with the copy button', async () => {
    const writeText = vi.fn(async () => Promise.resolve());
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText }
    });
    vi.stubGlobal('isSecureContext', true);
    render(Harness, { code });
    await fireEvent.click(screen.getByRole('button'));
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith(code));
    expect(await screen.findByText('Copied')).toBeInTheDocument();
    vi.unstubAllGlobals();
  });
});

describe('Code.Overflow', () => {
  it('starts collapsed and expands on click', async () => {
    const { container } = render(Harness, { code, overflow: true });
    const wrapper = container.querySelector('[data-code-overflow]')!;
    expect(wrapper).toHaveAttribute('data-collapsed', 'true');
    await fireEvent.click(screen.getByRole('button', { name: 'Expand' }));
    expect(wrapper).toHaveAttribute('data-collapsed', 'false');
    expect(screen.queryByRole('button', { name: 'Expand' })).toBeNull();
  });
});
