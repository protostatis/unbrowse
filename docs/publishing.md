# Publishing

This document records release and distribution workflows for `unbrowser`.
It is intentionally separate from `skills/unbrowser/SKILL.md`, which is user-facing.

## Artifact Matrix

- GitHub Release: automated in `.github/workflows/release.yml`
- PyPI (`pyunbrowser`): automated in `.github/workflows/release.yml`
- GHCR Docker image (`ghcr.io/protostatis/unbrowser`): automated in `.github/workflows/release.yml`
- crates.io (`unbrowser` crate): manual `cargo publish`
- Homebrew tap: manual update of `protostatis/homebrew-tap`
- ClawHub skill: manual `clawhub publish ...`
- Official MCP Registry: manual `mcp-publisher publish`; v0.0.18 is prepared, not published

## GitHub Release + PyPI

Use Python 3.11 or newer for `scripts/release_check.py`; it uses the standard
library `tomllib` parser.

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

1. Ensure `CARGO_REGISTRY_TOKEN` is available in the environment. For local releases, load it from the gitignored `.env` without printing it: `set -a; . ./.env; set +a`.
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

Canonical skill page: `https://clawhub.ai/protostatis/skills/unbrowser`.

## Official MCP Registry

Status for `0.0.18`: **prepared, not published**. The checked-in `server.json`
declares the PyPI and crates.io packages only; it intentionally does not declare
the hosted endpoint as a remote transport. Committing or merging this metadata
has no Registry side effect.

`mcp-publisher publish` is the external owner gate. Do not run it during normal
verification: a published Registry name/version is immutable, and hiding or
deleting a listing does not make that version reusable.

Owner-gated publish sequence:

1. Run `python3 scripts/release_check.py --tag v0.0.18 --strict-skill`,
   `python3 -m unittest discover -s tests -p 'test_release_check.py'`, and the
   build/test gates above.
2. Publish and verify both underlying `0.0.18` packages first: `pyunbrowser` on
   PyPI and `unbrowser` on crates.io. Their rendered package READMEs must expose
   `mcp-name: io.github.protostatis/unbrowser` before Registry publication.
   Also run the exact PyPI command derived from `server.json`,
   `uvx pyunbrowser==0.0.18 --mcp`, through an MCP initialize/tools smoke test;
   the `pyunbrowser` console-script alias exists specifically for this path.
3. Run `mcp-publisher validate`. This is a manifest/schema check; it does not
   prove end-to-end package ownership, so a passing result is not permission to
   publish.
4. When the owner is ready to make the irreversible submission, authenticate
   with `mcp-publisher login github`, inspect `server.json` one final time, and
   run `mcp-publisher publish` exactly once.
5. Verify the exact published record with the read-only endpoint:

   ```bash
   curl -fsS 'https://registry.modelcontextprotocol.io/v0.1/servers/io.github.protostatis%2Funbrowser/versions/0.0.18'
   ```

Until step 4 succeeds, describe this channel only as **prepared, not published**.

## Glama hosted MCP

Glama is a hosted MCP distribution channel. Keep it pinned to the same
`pyunbrowser` version as the repo release so the hosted Inspector demo matches
the local install path.

1. Confirm the new PyPI release has Linux wheels:
   `https://pypi.org/pypi/pyunbrowser/X.Y.Z/json`.
2. Update `docs/distribution.md` Glama build settings and smoke result.
3. Run `python3 scripts/release_check.py --tag vX.Y.Z`.
4. In Glama admin, update the Dockerfile build step:
   `uv venv /opt/unbrowser --python 3.14 && VIRTUAL_ENV=/opt/unbrowser uv pip install pyunbrowser==X.Y.Z`.
5. Click **Build & Release** and wait for a successful test.
6. Verify instance logs show `serverInfo.version = "X.Y.Z"` and
   `tools/list` returns the expected tools.
7. Run the hosted smoke checklist in `docs/distribution.md`.
