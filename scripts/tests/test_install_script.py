"""Tests for the one-line installer's user-facing text (scripts/install.sh).

Run: python3 -m unittest discover -s scripts/tests
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]


class InstallerTextTests(unittest.TestCase):
    def test_hotkey_hint_names_the_default_modifier(self) -> None:
        # The last thing a new Carbon reads must be the shortcut that actually opens a peek:
        # peek's default modifier (crates/client DEFAULT_HOTKEY_MODIFIER), not plain cmd, which
        # switches browser and editor tabs.
        identity = (REPO / "crates" / "client" / "src" / "identity.rs").read_text()
        match = re.search(r'pub const DEFAULT_HOTKEY_MODIFIER: &str = "([^"]+)";', identity)
        self.assertIsNotNone(match, "DEFAULT_HOTKEY_MODIFIER moved; update this test")
        assert match
        modifier = match[1]
        script = (REPO / "scripts" / "install.sh").read_text()
        self.assertIn(f"press {modifier}+<1-8>", script)
        self.assertNotRegex(script, r"press cmd\+<1-8>")


if __name__ == "__main__":
    unittest.main()
