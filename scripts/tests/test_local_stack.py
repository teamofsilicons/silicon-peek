"""Local stack credentials stay in the server's process environment."""

import importlib.util
import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "local_stack", Path(__file__).resolve().parents[1] / "e2e/local_stack.py"
)
local_stack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(local_stack)


class LocalStackTests(unittest.TestCase):
    def test_speech_keys_are_only_passed_to_server(self):
        keys = {"PEEK_DEEPGRAM_API_KEY": "fake-deepgram-local-stack-key", "PEEK_OPENAI_API_KEY": "fake-openai-local-stack-key"}
        children = {}

        def spawn(argv, **kwargs):
            children[Path(argv[0]).name] = kwargs["env"].copy()
            kwargs["stdout"].close()
            return SimpleNamespace(pid=100 + len(children))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = root / "bin"
            binaries.mkdir()
            for name in ("peek-server", "peekd", "peek"):
                (binaries / name).touch()
            sockets = root / "sockets"
            sockets.mkdir()
            output = io.StringIO()
            with (
                patch.dict(os.environ, keys, clear=True),
                patch.object(local_stack, "BIN", binaries),
                patch.object(local_stack, "CURRENT", root / "current"),
                patch.object(local_stack, "wait_until"),
                patch.object(local_stack.tempfile, "mkdtemp", return_value=str(sockets)),
                patch.object(local_stack.subprocess, "Popen", side_effect=spawn),
                patch.object(local_stack.subprocess, "run") as cargo,
                redirect_stdout(output),
            ):
                local_stack.build()
                self.assertNotIn("--bin", cargo.call_args.args[0])
                for name in keys:
                    self.assertNotIn(name, cargo.call_args.kwargs["env"])
                stack = local_stack.start(root / "stack", no_build=True)
            self.assertTrue(stack.state["elevenlabs"])
            self.assertTrue(stack.state["openai"])
            for name, key in keys.items():
                self.assertEqual(children["peek-server"][name], key)
                for child, env in children.items():
                    if child != "peek-server":
                        self.assertNotIn(name, env)
                self.assertNotIn(key, output.getvalue())
                for path in root.rglob("*"):
                    if path.is_file():
                        self.assertNotIn(key.encode(), path.read_bytes(), str(path))


if __name__ == "__main__":
    unittest.main()
