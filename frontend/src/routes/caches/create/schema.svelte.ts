import z from 'zod';

// Mirrors the backend `CacheRegex::is_valid`.
export const cacheName = z
  .string()
  .min(1, 'Cache name cannot be empty')
  .max(63, 'Cache name is too long (max 63 characters)')
  .regex(
    /^[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?$/,
    'Cache name may only contain letters, numbers and hyphens, and cannot start or end with a hyphen'
  )
  .refine((name) => !/^\d+$/.test(name), 'Cache name cannot be purely numeric');

export const information = z.object({
  name: cacheName.default(''),
  public: z.boolean().default(false),
  quota: z
    .number()
    .min(1, 'Quota must be a positive number')
    .default(10 * 1024),
  sig_key: z.string().min(1, 'Signature key is required').default('')
});
