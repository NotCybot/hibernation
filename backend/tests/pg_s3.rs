//! Production-like stack: the real nix client against a server on Postgres + S3 (testcontainers, needs Docker).
//! Skipped (passes with a note) when `nix` is not on PATH. Same tests as `nix_client.rs`, see `nix_common`.
mod common;
mod nix_common;

nix_client_tests!(common::TestServer::start_pg_s3().await);
