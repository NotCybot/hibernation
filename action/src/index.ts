import { run } from './run';

// Run never rejects: every error is routed to core.setFailed.
// oxlint-disable-next-line no-void
void run();
