import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Page from '$routes/+page.svelte';

const path = (name: string, accessed: number) => ({
  accessed,
  cache_name: 'main',
  cache_uuid: 'cache-main',
  created_at: new Date('2026-01-01T00:00:00Z'),
  last_accessed_at: new Date('2026-01-02T00:00:00Z'),
  size: 2048,
  store_path: `/nix/store/${'a'.repeat(32)}-${name}`
});

const data = (caches: unknown, tokens: unknown, overview: unknown) =>
  ({
    caches: Promise.resolve(caches),
    overview: Promise.resolve(overview),
    tokens: Promise.resolve(tokens)
  }) as never;

describe('overview page', () => {
  it('renders the Overview label while loading', () => {
    render(Page, {
      data: {
        caches: new Promise(() => {}),
        overview: new Promise(() => {}),
        tokens: new Promise(() => {})
      } as never
    });
    expect(screen.getByText('Overview')).toBeInTheDocument();
    expect(screen.getAllByText('Loading...')).toHaveLength(4);
  });

  it('shows caches, active tokens and paths', async () => {
    render(Page, {
      data: data(
        [
          {
            name: 'main',
            public: true,
            quota: 1024 ** 3,
            size: null,
            uuid: 'cache-main'
          }
        ],
        [
          {
            exp: new Date('2099-01-01'),
            last_used: null,
            name: 'ci',
            uuid: 't1'
          },
          {
            exp: new Date('2020-01-01'),
            last_used: null,
            name: 'old',
            uuid: 't2'
          }
        ],
        {
          most_accessed: [path('hello-2.12', 3)],
          recent_pushes: [path('ripgrep-14.1', 0)]
        }
      )
    });

    expect(await screen.findByText('ci')).toBeInTheDocument();
    expect(screen.queryByText('old')).toBeNull();
    expect(screen.getByText('hello-2.12')).toBeInTheDocument();
    expect(screen.getByText('ripgrep-14.1')).toBeInTheDocument();
    expect(screen.getByText('3×')).toBeInTheDocument();
  });

  it('shows empty states', async () => {
    render(Page, {
      data: data([], [], { most_accessed: [], recent_pushes: [] })
    });

    expect(await screen.findByText('No caches yet.')).toBeInTheDocument();
    expect(screen.getByText('No active tokens.')).toBeInTheDocument();
    expect(screen.getByText('Nothing pushed yet.')).toBeInTheDocument();
    expect(screen.getByText('Nothing pulled yet.')).toBeInTheDocument();
  });

  it('shows errors when the loads failed', async () => {
    render(Page, { data: data(undefined, undefined, undefined) });

    expect(
      await screen.findByText('Failed to load caches.')
    ).toBeInTheDocument();
    expect(screen.getByText('Failed to load tokens.')).toBeInTheDocument();
    expect(screen.getAllByText('Failed to load paths.')).toHaveLength(2);
  });
});
