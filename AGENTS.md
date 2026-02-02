# Repository Guidelines

## Project Structure & Module Organization
- `crates/` contains workspace crates (e.g., `indexer-agent`, `indexer-service-rs`, `indexer-tap-agent`, `indexer-management-api`). Each crate owns its domain logic and tests.
- `migrations/` holds SQLx migrations shared across services.
- `integration-tests/` provides end-to-end scenarios and load tests that assume a running local network.
- `docs/` contains design notes and contributor-facing docs.
- `contrib/` and `contrib/local-network/` hold Docker Compose setups and network tooling.
- `setup-test-network.sh` and `run_network.sh` bootstrap the local-network stack.
- `docs/alloy-standards.md` defines Alloy usage standards and phased implementation.

## Build, Test, and Development Commands
Use `just` for common workflows (see `justfile`):
- `just fmt` runs `cargo +nightly fmt`.
- `just clippy` runs `cargo +nightly clippy --all-targets --all-features`.
- `just test` runs `cargo nextest run` (workspace tests).
- `just sqlx-prepare` generates offline SQLx metadata.
- `just setup` boots the local network and services for integration tests.
- `just test-local` / `just test-local-v2` run TAP integration tests against the local network.
- `just phase4` runs the Phase 4 local-network execution script.
- `just alloy-standards` prints the Alloy standards doc.

Direct cargo examples:
- `cargo build` (workspace build)
- `cargo test -p indexer-agent` (crate-specific tests)
- `SQLX_OFFLINE=true cargo test` (offline SQLx metadata)

## Coding Style & Naming Conventions
- Rust formatting uses `rustfmt` (see `rustfmt.toml`).
- Use standard Rust naming: `snake_case` for functions/vars, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Prefer explicit types for protocol identifiers (deployment IDs, allocation IDs, chain IDs) where possible.
- Follow `docs/alloy-standards.md` for Alloy-specific conventions and phase gates.

## Testing Guidelines
- Unit and integration tests are Rust `#[test]`/`#[tokio::test]`.
- Snapshot tests use `insta` (review with `just review`).
- End-to-end tests live in `integration-tests/` and require the local network.
- Keep tests deterministic; avoid relying on external networks.

## Commit & Pull Request Guidelines
- Commit messages follow Conventional Commits with scopes, e.g. `fix(agent): ...`, `feat(service): ...`, `test(tap-agent): ...`.
- PRs should include: summary of changes, test commands run, and any migration/config impact. Link related issues when available.

## Security & Configuration Tips
- Database access uses SQLx; set `DATABASE_URL` when running migrations or local services.
- Local network scripts expect `integration-tests/.env` (use `just setup-integration-env`).
- Do not commit secrets; use `.env` or local config files.
