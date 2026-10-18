import * as core from '@actions/core';
import * as tc from '@actions/tool-cache';
import { execFileSync } from 'node:child_process';

const REPO = 'ProfiiDev/hibernation',
  OS: Partial<Record<NodeJS.Platform, string>> = {
    darwin: 'macos',
    linux: 'linux'
  },
  ARCH: Partial<Record<NodeJS.Architecture, string>> = {
    arm64: 'arm64',
    x64: 'x86-64'
  },
  // Args go straight to execve, never through a shell, so inputs can't inject commands.
  hibernation = (args: string[]) => {
    try {
      execFileSync('hibernation', args, { stdio: 'inherit' });
    } catch (error) {
      // Don't surface execFileSync's message: it echoes the full argv, including the token.
      const status =
        error instanceof Error && 'status' in error
          ? String(error.status)
          : 'unknown';
      throw new Error(`hibernation ${args[0]} failed (exit code ${status})`, {
        cause: error
      });
    }
  },
  install = async (version: string) => {
    const { platform, arch } = process,
      osPart = OS[platform],
      archPart = ARCH[arch];
    if (!osPart) {
      throw new Error(`Unsupported platform: ${platform}`);
    }
    if (!archPart) {
      throw new Error(`Unsupported architecture: ${arch}`);
    }

    const suffix = platform === 'linux' ? '-gnu' : '',
      assetName = `hibernation-${osPart}-${archPart}${suffix}.tar.gz`,
      downloadUrl =
        version === 'latest'
          ? `https://github.com/${REPO}/releases/latest/download/${assetName}`
          : `https://github.com/${REPO}/releases/download/${version}/${assetName}`;

    core.info(`Downloading ${assetName} from ${downloadUrl}`);
    const pathToTarball = await tc.downloadTool(downloadUrl),
      pathToCli = await tc.extractTar(pathToTarball);
    core.addPath(pathToCli);
    core.info('Successfully installed hibernation CLI');
  },
  configure = (url: string, token: string, signingKey: string) => {
    if (url && token) {
      hibernation(['auth', '--url', url, token]);
      core.info('Configured hibernation CLI with provided URL and token');
    } else if (url) {
      hibernation(['set-url', url]);
      core.info('Configured hibernation CLI with provided URL');
    } else if (token) {
      core.warning('Token provided without URL, skipping configuration');
    }

    if (signingKey) {
      core.exportVariable('HIBERNATION_SIGNING_KEY', signingKey);
      core.info('Set signing key for hibernation CLI');
    }
  };

export const run = async (): Promise<void> => {
  try {
    const version = core.getInput('version') || 'latest',
      url = core.getInput('url'),
      token = core.getInput('token'),
      signingKey = core.getInput('signing-key');
    if (token) {
      core.setSecret(token);
    }
    if (signingKey) {
      core.setSecret(signingKey);
    }

    await install(version);
    configure(url, token, signingKey);
  } catch (error) {
    core.setFailed(error instanceof Error ? error.message : String(error));
  }
};
