import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';

const newCode = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  newCode: () => newCode()
}));
const sendCliCode = vi.fn();
vi.mock('$lib/backend/util.svelte.js', () => ({
  sendCliCode: (c: string) => sendCliCode(c)
}));

const Page = (await import('$routes/auth/cli/+page.svelte')).default;

const close = vi.spyOn(window, 'close').mockImplementation(() => {});

afterEach(() => {
  newCode.mockReset();
  sendCliCode.mockReset();
  close.mockClear();
});

describe('cli auth page', () => {
  it('shows the requesting state until a code arrives', () => {
    newCode.mockReturnValue(new Promise(() => {}));
    render(Page);
    expect(
      screen.getByText('Requesting new CLI auth code...')
    ).toBeInTheDocument();
  });

  it('sends the code to the cli and closes the window on success', async () => {
    newCode.mockResolvedValue({ data: { code: 'abc123' } });
    sendCliCode.mockResolvedValue(undefined);
    render(Page);
    expect(
      await screen.findByText(
        'CLI authenticated successfully! You can now close this window.'
      )
    ).toBeInTheDocument();
    expect(sendCliCode).toHaveBeenCalledWith('abc123');
    expect(close).toHaveBeenCalledOnce();
  });

  it('shows the code while the cli is being contacted', async () => {
    newCode.mockResolvedValue({ data: { code: 'abc123' } });
    sendCliCode.mockReturnValue(new Promise(() => {}));
    render(Page);
    expect(
      await screen.findByText('Trying to authenticate CLI with code:')
    ).toBeInTheDocument();
    expect(screen.getByDisplayValue('abc123')).toHaveAttribute('readonly');
  });

  it('falls back to manual entry when the cli is unreachable', async () => {
    newCode.mockResolvedValue({ data: { code: 'abc123' } });
    sendCliCode.mockResolvedValue('Other');
    render(Page);
    expect(
      await screen.findByText('Failed to send code. Please enter it manually:')
    ).toBeInTheDocument();
    expect(screen.getByDisplayValue('abc123')).toBeInTheDocument();
    expect(close).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: /try again/i })).toBeNull();
  });

  it('shows an error with retry when no code can be created, and retries', async () => {
    newCode.mockResolvedValueOnce({ error: 'x' });
    render(Page);
    expect(
      await screen.findByText('Failed to get CLI auth code')
    ).toBeInTheDocument();
    expect(sendCliCode).not.toHaveBeenCalled();

    newCode.mockResolvedValueOnce({ data: { code: 'zzz999' } });
    sendCliCode.mockResolvedValue(undefined);
    await fireEvent.click(screen.getByRole('button', { name: /try again/i }));
    expect(
      await screen.findByText(
        'CLI authenticated successfully! You can now close this window.'
      )
    ).toBeInTheDocument();
    expect(sendCliCode).toHaveBeenCalledWith('zzz999');
    expect(screen.queryByRole('button', { name: /try again/i })).toBeNull();
  });
});
