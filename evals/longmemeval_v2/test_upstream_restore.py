import os
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest


@unittest.skipUnless(
    os.getenv("LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY"),
    "requires pinned upstream checkout on PYTHONPATH and a native product binary",
)
class UpstreamRestoreTests(unittest.TestCase):
    def test_saved_store_reopens_after_original_store_moves(self):
        from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend
        from memory_modules.memory import load_memory, save_memory

        with TemporaryDirectory() as directory:
            root = Path(directory)
            original = root / "original"
            backend = MemoryMcpBackend({
                "binary": os.environ["LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY"],
                "db_path": str(original),
            })
            backend.insert({"id": "synthetic-tea", "content": "I prefer tea"})
            snapshot = root / "snapshot"
            save_memory(backend, snapshot)
            original.rename(root / "moved")

            restored = load_memory(snapshot)
            self.addCleanup(restored._restored_store.cleanup)
            items = restored.query("What do I prefer?")

            self.assertTrue(any("tea" in item["value"].lower() for item in items))
            self.assertNotEqual(Path(restored.db_path), original)
            self.assertFalse(original.exists())
