# Contributing to Unbrowser

Thanks for helping improve Unbrowser. Small, focused changes with a clear
reproduction or test are the easiest to review.

## Before opening an issue

- Search the existing issues first.
- Include the Unbrowser version, install path, operating system, and a minimal
  public-data reproduction.
- Remove credentials, cookies, tokens, and private data from logs.
- Report suspected vulnerabilities through [the private security
  process](SECURITY.md), not a public issue.

## Local setup

The native binary uses the stable Rust toolchain. Building the current HTTP
stack also requires CMake and Ninja. Release validation uses Python 3.11 or
newer.

```bash
cargo build --locked
```

See [the source-build instructions](docs/usage.md#build-from-source) for
platform-specific prerequisites.

## Verification

Run the checks relevant to the change. Before submitting a code change, run
the full local baseline:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
python3 -m unittest discover -s tests -p 'test_*.py'
python3 scripts/release_check.py
```

Behavior changes should also include or update a focused test or smoke script.
Tests that require external sites must document that dependency and fail with
enough context to distinguish a product regression from a site or network
change.

## Pull requests

- Keep each pull request scoped to one problem.
- Explain the user-visible behavior before and after the change.
- List the exact verification commands and results.
- Update documentation when an interface, limitation, or escalation boundary
  changes.
- Do not commit generated binaries, credentials, cookies, tokens, private
  captures, or `.env` files.

Release publication and registry updates remain maintainer operations; see
[the publishing guide](docs/publishing.md) for the release contract.
