from __future__ import annotations

import unittest

from scripts.hosted_mcp_smoke import status_payload_is_ready


class HostedMcpStatusPayloadTest(unittest.TestCase):
    def test_accepts_legacy_configured_instance(self) -> None:
        self.assertTrue(status_payload_is_ready({"server_instances": {"default": "configured"}}))

    def test_accepts_current_capacity_contract(self) -> None:
        self.assertTrue(status_payload_is_ready({"status": "ok", "active_sessions": 2, "capacity": 8}))

    def test_rejects_unhealthy_or_malformed_capacity_contracts(self) -> None:
        invalid_payloads = [
            {"status": "error", "active_sessions": 0, "capacity": 8},
            {"status": "ok", "active_sessions": -1, "capacity": 8},
            {"status": "ok", "active_sessions": 9, "capacity": 8},
            {"status": "ok", "active_sessions": 0, "capacity": 0},
            {"status": "ok", "active_sessions": True, "capacity": 8},
            {"status": "ok", "active_sessions": 0, "capacity": "8"},
            {"status": "ok"},
            {},
        ]

        for payload in invalid_payloads:
            with self.subTest(payload=payload):
                self.assertFalse(status_payload_is_ready(payload))


if __name__ == "__main__":
    unittest.main()
