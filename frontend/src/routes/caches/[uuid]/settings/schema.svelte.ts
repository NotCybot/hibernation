import { EvictionPolicy } from '$lib/client';
import { z } from 'zod';
import { cacheName } from '../../create/schema.svelte';

export const cacheSchema = z.object({
  allow_force_push: z.boolean().default(false),
  eviction_policy: z
    .array(z.enum(EvictionPolicy))
    .default([EvictionPolicy.LEAST_RECENTLY_USED]),
  name: cacheName.default(''),
  priority: z
    .number()
    .int()
    .min(0, { message: 'Priority must be a non-negative integer' })
});

export const quotaSchema = z.object({
  quota: z
    .number()
    .min(1, 'Quota must be a positive number')
    .default(10 * 1024)
});

export const keySchema = z.object({
  sig_key: z.string().min(1, 'Signature key is required').default('')
});
