"""Pure/negative smoke-check tests: no local sockets or privileged calls are made."""

import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("netns_smoke", Path(__file__).with_name("netns-smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)

STATUS = """Uid: 1001 1001 1001 1001
Gid: 1002 1002 1002 1002
Groups:
NoNewPrivs: 1
CapInh: 0000000000000000
CapPrm: 0000000000000000
CapEff: 0000000000000000
CapBnd: 0000000000000000
CapAmb: 0000000000000000
"""


class SmokeTests(unittest.TestCase):
    def test_expected_process_privileges_are_accepted(self):
        smoke.inspect_process(STATUS, 1001, 1002)

    def test_root_saved_identity_groups_or_capability_leaks_are_rejected(self):
        changes = [("Uid: 1001 1001 1001 1001", "Uid: 1001 1001 0 1001"),
                   ("Gid: 1002 1002 1002 1002", "Gid: 1002 1002 0 1002"),
                   ("Groups:", "Groups: 999"), ("NoNewPrivs: 1", "NoNewPrivs: 0")]
        changes += [(f"{name}: 0000000000000000", f"{name}: 0000000000000001")
                    for name in ("CapInh","CapPrm","CapEff","CapBnd","CapAmb")]
        for before, after in changes:
            with self.subTest(field=before):
                with self.assertRaises(RuntimeError):
                    smoke.inspect_process(STATUS.replace(before, after),1001,1002)

    def test_only_loopback_interfaces_and_routes_are_accepted(self):
        smoke.inspect_network([(1,"lo")],"Iface Destination\n","")
        for interfaces, routes, ipv6 in [
            ([(1,"lo"),(2,"eth0")],"Iface Destination\n",""),
            ([(1,"lo")],"Iface Destination\neth0 00000000\n",""),
            ([(1,"lo")],"Iface Destination\n","0000 eth0\n"),
        ]:
            with self.subTest(interfaces=interfaces,routes=routes,ipv6=ipv6):
                with self.assertRaises(RuntimeError):
                    smoke.inspect_network(interfaces,routes,ipv6)

    def test_non_linux_refuses_before_any_socket_creation(self):
        with patch.object(smoke,"sys",SimpleNamespace(platform="darwin")):
            with patch.object(smoke,"check_sockets") as sockets:
                with self.assertRaises(RuntimeError):
                    smoke.run_checks(1001,1002)
                sockets.assert_not_called()

    def test_accessible_docker_socket_refuses_before_any_network_probe(self):
        proc = SimpleNamespace(read_text=lambda: STATUS)
        fake_os = SimpleNamespace(access=lambda *_: True, R_OK=4, W_OK=2)
        with patch.object(smoke,"sys",SimpleNamespace(platform="linux")), \
             patch.object(smoke,"Path",return_value=proc), patch.object(smoke,"os",fake_os), \
             patch.object(smoke,"check_sockets") as sockets:
            with self.assertRaisesRegex(RuntimeError,"Docker"):
                smoke.run_checks(1001,1002)
            sockets.assert_not_called()


if __name__ == "__main__":
    unittest.main()
