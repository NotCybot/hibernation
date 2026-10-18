//! End to end against the real nix client (sqlite + local storage): the responses must be accepted by `nix` itself.
//! Skipped (passes with a note) when `nix` is not on PATH. The tests live in `nix_common`, shared with `pg_s3.rs`.
mod common;
mod nix_common;

nix_client_tests!(common::TestServer::start().await);
