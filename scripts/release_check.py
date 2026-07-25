#!/usr/bin/env python3
"""Release/documentation drift checks for unbrowser."""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]


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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="Optional release tag, e.g. v0.0.13")
    parser.add_argument("--strict-skill", action="store_true", help="Require skill version to equal binary version")
    args = parser.parse_args()

    cargo = tomllib.loads(read("Cargo.toml"))["package"]["version"]
    cargo_lock = version_from_cargo_lock()
    pyproject = tomllib.loads(read("python/pyproject.toml"))["project"]["version"]
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

    distribution_versions = pinned_pyunbrowser_versions("docs/distribution.md")
    require(
        version in distribution_versions,
        f"docs/distribution.md Glama pyunbrowser pins {distribution_versions!r} do not include {version}",
    )

    skill_version = version_from_skill()
    if args.strict_skill:
        require(skill_version == version, f"skill version {skill_version!r} != binary version {version!r}")

    readme = read("README.md")
    skill = read("skills/unbrowser/SKILL.md")
    pycli = read("python/unbrowser/_cli.py")

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
        require(needle in readme, f"README missing {needle!r}")
        require(needle in skill, f"skill docs missing {needle!r}")

    require("unbrowser session start" in pycli, "Python CLI help missing session start")
    require("session prune" in pycli, "Python CLI help missing session prune")
    require("--pretty" in pycli, "Python CLI help missing --pretty")

    session_pos = readme.find("Session CLI")
    bare_rpc_pos = readme.find("Bare RPC")
    require(session_pos != -1, "README missing Session CLI section")
    require(bare_rpc_pos == -1 or session_pos < bare_rpc_pos, "README should present session CLI before bare RPC")

    print({"version": version, "skill_version": skill_version, "ok": True})
    return 0


if __name__ == "__main__":
    sys.exit(main())
