#!/usr/bin/env python3
"""Claim rules of scripts/dev-slots.py. Run: python3 scripts/dev-slots.test.py

Each test gets its own registry (OXY_DEV_HOME) and checkout directories, and a slot
stride far from the dev ports, so it never touches a real stack's registry or ports.
"""

import importlib.util
import os
import socket
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    "dev_slots", os.path.join(HERE, "dev-slots.py")
)
slots = importlib.util.module_from_spec(spec)
spec.loader.exec_module(slots)


def free_base():
    """A run of ports nothing listens on, so `ports_free` holds for the first slots."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1] // 100 * 100 - 2000


class ClaimTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.environ["OXY_DEV_HOME"] = os.path.join(self.tmp.name, "home")
        os.environ.pop("OXY_DEV_SLOT", None)
        os.environ["OXY_DEV_MAX_SLOTS"] = "4"
        base = free_base()
        slots.API_BASE, slots.INTERNAL_BASE, slots.FRONTEND_BASE = (
            base,
            base + 1,
            base + 50,
        )

    def repo(self, name):
        path = os.path.join(self.tmp.name, name)
        os.makedirs(path, exist_ok=True)
        return os.path.realpath(path)

    def claim(self, repo, exclude=()):
        return slots.choose(repo, set(exclude))["slot"]

    def test_a_checkout_keeps_its_slot(self):
        a, b = self.repo("a"), self.repo("b")
        self.assertEqual(self.claim(a), 0)
        self.assertEqual(self.claim(b), 1)
        self.assertEqual(self.claim(a), 0)
        self.assertEqual(self.claim(b), 1)

    def test_ports_follow_the_slot(self):
        p = slots.ports(2)
        self.assertEqual(p["api"] - slots.API_BASE, 20)
        self.assertEqual(p["internal"], p["api"] + 1)
        self.assertEqual(p["frontend"] - slots.FRONTEND_BASE, 20)

    def test_a_busy_port_skips_the_slot(self):
        with socket.socket() as busy:
            busy.bind(("127.0.0.1", slots.ports(0)["frontend"]))
            busy.listen()
            self.assertEqual(self.claim(self.repo("a")), 1)

    def test_moving_releases_the_old_slot(self):
        a = self.repo("a")
        self.assertEqual(self.claim(a), 0)
        self.assertEqual(self.claim(a, exclude=[0]), 1)
        self.assertIsNone(slots.read_slot(0))
        self.assertEqual(self.claim(self.repo("b")), 0)

    def test_a_deleted_checkout_frees_its_slot(self):
        a, b = self.repo("a"), self.repo("b")
        self.claim(a)
        os.rmdir(a)
        self.assertEqual(self.claim(b), 0)

    def test_when_full_the_stalest_idle_slot_is_taken(self):
        repos = [self.repo(f"r{i}") for i in range(4)]
        for r in repos:
            self.claim(r)
        self.claim(repos[0])  # r1 is now the least recently used
        self.assertEqual(self.claim(self.repo("late")), 1)

    def test_full_of_running_stacks_refuses(self):
        for i in range(4):
            self.claim(self.repo(f"r{i}"))
        sleeper = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)", "--strictPort"],
            start_new_session=True,
        )
        self.addCleanup(sleeper.kill)
        for n in range(4):
            rec = slots.read_slot(n)
            rec["frontend"] = {"pid": sleeper.pid, "url": "http://127.0.0.1:1"}
            slots.write_slot(rec)
        with self.assertRaises(SystemExit):
            self.claim(self.repo("late"))

    def test_a_dead_pid_is_not_a_running_stack(self):
        rec = {"backend": {"pid": 2**22 + 12345}, "frontend": None}
        self.assertFalse(slots.live(rec))


if __name__ == "__main__":
    unittest.main()
