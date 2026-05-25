# Publishing

This document records release and distribution workflows for `unbrowser`.
It is intentionally separate from `skills/unbrowser/SKILL.md`, which is user-facing.

## Artifact Matrix

- GitHub Release: automated in `.github/workflows/release.yml`
- PyPI (`pyunbrowser`): automated in `.github/workflows/release.yml`
- crates.io (`unbrowser` crate): manual `cargo publish`
- Homebrew tap: manual update of `protostatis/homebrew-tap`
- ClawHub skill: manual `clawhub publish ...`

## GitHub Release + PyPI

1. Bump `Cargo.toml`, `Cargo.lock`, `python/pyproject.toml`, and
   `python/unbrowser/__init__.py` together.
2. Run `python3 scripts/release_check.py --tag vX.Y.Z`, `cargo build --release --locked`, and `cargo test`.
3. Commit and push to `main`.
4. Tag the release: `git tag -a vX.Y.Z -m "..." && git push origin vX.Y.Z`.
5. The `release.yml` workflow builds the binaries, creates the GitHub Release,
   and publishes Python wheels to PyPI via OIDC trusted publishing. It does
   not publish an sdist until source installs can build the native binary.

After publish, verify the installed surfaces that agents use first:

```bash
unbrowser --version
unbrowser --help
unbrowser session --help
unbrowser navigate https://news.ycombinator.com --json
```

## crates.io

1. Ensure `CARGO_REGISTRY_TOKEN` is available in the environment.
2. Bump `Cargo.toml` version.
3. Run `cargo publish` from the repo root.

## Homebrew tap

Repo: `https://github.com/protostatis/homebrew-tap`

1. Release `unbrowser` first so the GitHub Release tarballs exist.
2. Update or clone the tap repo.
3. Run `./bin/update-shas.sh vX.Y.Z` in the tap repo.
4. Review the diff in `Formula/unbrowser.rb`, then commit and push.

The tap repo’s `bin/update-shas.sh` helper fetches the tarballs for the given
tag, computes the sha256s, updates `Formula/unbrowser.rb`, and prints a diff.

## ClawHub skill

1. Bump `skills/unbrowser/SKILL.md` `version:` frontmatter.
2. Commit and push the skill change.
3. Publish manually:

```bash
clawhub publish skills/unbrowser --version X.Y.Z --changelog "..."
```

ClawHub does not auto-sync from GitHub, and it requires `--version` explicitly.
It will reject a reused version with `Version already exists`.

## Glama hosted MCP

Glama is a hosted MCP distribution channel. Keep it pinned to the same
`pyunbrowser` version as the repo release so the hosted Inspector demo matches
the local install path.

1. Confirm the new PyPI release has Linux wheels:
   `https://pypi.org/pypi/pyunbrowser/X.Y.Z/json`.
2. Update the root `Dockerfile` pin to `pyunbrowser==X.Y.Z`.
3. Update `docs/distribution.md` Glama build settings and smoke result.
4. Run `python3 scripts/release_check.py --tag vX.Y.Z`.
5. In Glama admin, update the Dockerfile build step:
   `uv venv /opt/unbrowser --python 3.14 && VIRTUAL_ENV=/opt/unbrowser uv pip install pyunbrowser==X.Y.Z`.
6. Click **Build & Release** and wait for a successful test.
7. Verify instance logs show `serverInfo.version = "X.Y.Z"` and
   `tools/list` returns the expected tools.
8. Run the hosted smoke checklist in `docs/distribution.md`.
