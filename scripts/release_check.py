#!/usr/bin/env python3
"""Release/documentation drift checks for unbrowser."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]
REGISTRY_SCHEMA = "https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json"
REGISTRY_NAME = "io.github.protostatis/unbrowser"
REGISTRY_OWNERSHIP_MARKER = f"mcp-name: {REGISTRY_NAME}"
CARGO_OWNERSHIP_LINE = f"**Official MCP Registry identity:** `{REGISTRY_OWNERSHIP_MARKER}`"
PYPI_OWNERSHIP_MARKER = f"<!-- {REGISTRY_OWNERSHIP_MARKER} -->"
CANONICAL_CLAWHUB_URL = "https://clawhub.ai/protostatis/skills/unbrowser"
PYTHON_CLI_ENTRY_POINT = "unbrowser._cli:main"


def read(path: str) -> str:
    return (ROOT / path).read_text()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(message)


def version_from_init() -> str:
    match = re.search(r'^__version__ = "([^"]+)"', read("python/unbrowser/__init__.py"), re.M)
    require(match is not None, "python/unbrowser/__init__.py has no __version__")
    return match.group(1)


def version_from_skill() -> str | None:
    match = re.search(r"^version:\s*([0-9]+\.[0-9]+\.[0-9]+)", read("skills/unbrowser/SKILL.md"), re.M)
    return match.group(1) if match else None


def version_from_cargo_lock() -> str:
    lock = tomllib.loads(read("Cargo.lock"))
    matches = [
        pkg.get("version")
        for pkg in lock.get("package", [])
        if pkg.get("name") == "unbrowser" and pkg.get("version") is not None
    ]
    require(len(matches) == 1, f"Cargo.lock should contain exactly one unbrowser package, got {matches!r}")
    version = matches[0]
    require(isinstance(version, str), "Cargo.lock unbrowser package has no string version")
    return version


def pinned_pyunbrowser_versions(path: str) -> list[str]:
    return re.findall(r"pyunbrowser==([0-9]+\.[0-9]+\.[0-9]+)", read(path))


def validate_python_entry_points(project: object) -> None:
    require(isinstance(project, dict), "python/pyproject.toml [project] must be a table")
    scripts = project.get("scripts")
    require(isinstance(scripts, dict), "python/pyproject.toml [project.scripts] must be a table")
    for command in ("unbrowser", "pyunbrowser"):
        require(
            scripts.get(command) == PYTHON_CLI_ENTRY_POINT,
            f"python/pyproject.toml must expose {command!r} as {PYTHON_CLI_ENTRY_POINT!r}",
        )


def validate_registry_manifest(manifest: object, version: str) -> None:
    require(isinstance(manifest, dict), "server.json must contain a JSON object")
    expected_metadata = {
        "$schema": REGISTRY_SCHEMA,
        "name": REGISTRY_NAME,
        "title": "unbrowser by Unchained",
        "description": "Chrome-free MCP web access for agents with low-token page maps and browser escalation hints.",
        "repository": {
            "url": "https://github.com/protostatis/unbrowser",
            "source": "github",
            "id": "1226093137",
        },
        "version": version,
        "websiteUrl": (
            "https://unchainedsky.com/unbrowser?ref=official_mcp_registry"
            "&utm_source=official_mcp_registry&utm_medium=mcp_directory"
            "&utm_campaign=unbrowser_registry_v0019_launch"
        ),
    }
    for field, expected in expected_metadata.items():
        require(manifest.get(field) == expected, f"server.json {field} {manifest.get(field)!r} != {expected!r}")

    require("remotes" not in manifest, "server.json must not contain remotes for this package-only release")
    expected_packages = [
        {
            "registryType": "pypi",
            "registryBaseUrl": "https://pypi.org",
            "identifier": "pyunbrowser",
            "version": version,
            "runtimeHint": "uvx",
            "transport": {"type": "stdio"},
            "packageArguments": [{"type": "positional", "value": "--mcp"}],
        },
        {
            "registryType": "cargo",
            "registryBaseUrl": "https://crates.io",
            "identifier": "unbrowser",
            "version": version,
            "transport": {"type": "stdio"},
            "packageArguments": [{"type": "positional", "value": "--mcp"}],
        },
    ]
    require(
        manifest.get("packages") == expected_packages,
        f"server.json packages must match the pyunbrowser and unbrowser {version} stdio launch metadata",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="Optional release tag, e.g. v0.0.13")
    parser.add_argument("--strict-skill", action="store_true", help="Require skill version to equal binary version")
    args = parser.parse_args()

    cargo = tomllib.loads(read("Cargo.toml"))["package"]["version"]
    cargo_lock = version_from_cargo_lock()
    python_project = tomllib.loads(read("python/pyproject.toml"))["project"]
    validate_python_entry_points(python_project)
    pyproject = python_project["version"]
    module = version_from_init()
    versions = {
        "Cargo.toml": cargo,
        "Cargo.lock": cargo_lock,
        "python/pyproject.toml": pyproject,
        "python/unbrowser/__init__.py": module,
    }

    if args.tag:
        require(re.fullmatch(r"v\d+\.\d+\.\d+", args.tag) is not None, f"release tag must look like vX.Y.Z, got {args.tag!r}")
        versions["tag"] = args.tag[1:]

    require(len(set(versions.values())) == 1, "version mismatch: " + repr(versions))
    version = cargo

    try:
        registry_manifest = json.loads(read("server.json"))
    except json.JSONDecodeError as exc:
        raise SystemExit(f"server.json is invalid JSON: {exc}") from exc
    validate_registry_manifest(registry_manifest, version)

    distribution_versions = pinned_pyunbrowser_versions("docs/distribution.md")
    require(
        version in distribution_versions,
        f"docs/distribution.md Glama pyunbrowser pins {distribution_versions!r} do not include {version}",
    )

    skill_version = version_from_skill()
    if args.strict_skill:
        require(skill_version == version, f"skill version {skill_version!r} != binary version {version!r}")

    readme = read("README.md")
    usage = read("docs/usage.md")
    python_readme = read("python/README.md")
    distribution = read("docs/distribution.md")
    publishing = read("docs/publishing.md")
    skill = read("skills/unbrowser/SKILL.md")
    pycli = read("python/unbrowser/_cli.py")

    require(
        CARGO_OWNERSHIP_LINE in readme,
        f"README missing visible crates.io ownership marker {REGISTRY_OWNERSHIP_MARKER!r}",
    )
    require(
        PYPI_OWNERSHIP_MARKER in python_readme,
        f"python/README.md missing PyPI ownership marker {PYPI_OWNERSHIP_MARKER!r}",
    )
    for path, contents in [("docs/distribution.md", distribution), ("docs/publishing.md", publishing)]:
        require(CANONICAL_CLAWHUB_URL in contents, f"{path} missing canonical ClawHub URL")
        require("https://clawhub.com/skills/unbrowser" not in contents, f"{path} contains stale ClawHub URL")

    require(
        "https://unchainedsky.com/unbrowser?utm_source=github&utm_medium=repository&utm_campaign=unbrowser_readme&ref=readme_live_demo"
        in readme,
        "README missing attributed live Unbrowser demo URL",
    )

    for needle in [
        "unbrowser session start",
        "unbrowser exec",
        "session prune",
        "--pretty",
        "query_debug",
        "table_to_json",
        "challenge.provider",
    ]:
        require(needle in usage, f"docs/usage.md missing {needle!r}")
        require(needle in skill, f"skill docs missing {needle!r}")

    require("unbrowser session start" in pycli, "Python CLI help missing session start")
    require("session prune" in pycli, "Python CLI help missing session prune")
    require("--pretty" in pycli, "Python CLI help missing --pretty")

    require("docs/usage.md" in readme, "README missing usage-reference link")
    require("docs/compatibility.md" in readme, "README missing compatibility-reference link")
    session_pos = usage.find("Session CLI")
    bare_rpc_pos = usage.find("Raw JSON-RPC")
    require(session_pos != -1, "docs/usage.md missing Session CLI section")
    require(bare_rpc_pos == -1 or session_pos < bare_rpc_pos, "usage reference should present Session CLI before raw JSON-RPC")

    print({"version": version, "skill_version": skill_version, "ok": True})
    return 0


if __name__ == "__main__":
    sys.exit(main())
