from __future__ import annotations

import copy
import json
import tomllib
import unittest

from scripts import release_check


class RegistryManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = json.loads(release_check.read("server.json"))
        self.python_project = tomllib.loads(release_check.read("python/pyproject.toml"))["project"]

    def test_checked_in_manifest_matches_release_contract(self) -> None:
        release_check.validate_registry_manifest(self.manifest, "0.0.17")

    def test_python_distribution_exposes_registry_and_primary_commands(self) -> None:
        release_check.validate_python_entry_points(self.python_project)

    def test_missing_registry_command_is_rejected(self) -> None:
        project = copy.deepcopy(self.python_project)
        del project["scripts"]["pyunbrowser"]
        with self.assertRaisesRegex(SystemExit, "must expose 'pyunbrowser'"):
            release_check.validate_python_entry_points(project)

    def test_remote_transport_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["remotes"] = [{"type": "streamable-http", "url": "https://example.com/mcp"}]
        with self.assertRaisesRegex(SystemExit, "must not contain remotes"):
            release_check.validate_registry_manifest(manifest, "0.0.17")

    def test_package_version_drift_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["packages"][0]["version"] = "0.0.16"
        with self.assertRaisesRegex(SystemExit, "packages must match"):
            release_check.validate_registry_manifest(manifest, "0.0.17")

    def test_mcp_argument_drift_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.manifest)
        manifest["packages"][1]["packageArguments"][0]["value"] = "--version"
        with self.assertRaisesRegex(SystemExit, "packages must match"):
            release_check.validate_registry_manifest(manifest, "0.0.17")


if __name__ == "__main__":
    unittest.main()
