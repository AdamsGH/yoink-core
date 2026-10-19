import json
import sqlite3
import subprocess
import sys
import unittest
from pathlib import Path


class SnapshotDiagnostics(unittest.TestCase):
    def setUp(self):
        self.snapshot = Path("/snapshots/restored-test.db")
        for path in Path("/snapshots").iterdir():
            path.unlink()
        with sqlite3.connect(self.snapshot) as db:
            db.executescript(Path("/queries/fixture.sql").read_text())

    def query(self, name="latest", **filters):
        return subprocess.run(
            [sys.executable, "-I", "-B", "/queries/query-snapshot.py"],
            input=json.dumps({"snapshot": name, "match": "", **filters}),
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )

    def test_metadata_only_with_brevity_and_receipt(self):
        result = self.query()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("SENTINEL", result.stdout)
        report = json.loads(result.stdout)
        self.assertEqual(report["schema"], {"version": 27, "failed": 0})
        row = report["deliveries"][0]
        self.assertEqual(row["receipt"], 23)
        self.assertTrue(row["focus_requests_brevity"])
        self.assertEqual(row["alias"], "nobullshit")
        self.assertEqual(len(row["text_sha256"]), 64)

    def test_symlink_and_wal_are_rejected(self):
        Path("/snapshots/restored-link.db").symlink_to(self.snapshot)
        self.assertNotEqual(self.query("restored-link.db").returncode, 0)
        Path(str(self.snapshot) + "-wal").touch()
        self.assertNotEqual(self.query("restored-test.db").returncode, 0)

    def test_active_name_and_traversal_are_rejected(self):
        for name in ["native.db", "../restored-test.db"]:
            self.assertNotEqual(self.query(name).returncode, 0)

    def test_request_mapping_limits_snapshot_rows(self):
        selected = json.loads(self.query(chat_id=42, input_message_id=22).stdout)
        self.assertEqual(len(selected["deliveries"]), 1)
        unrelated = json.loads(self.query(chat_id=43, input_message_id=22).stdout)
        self.assertEqual(unrelated["deliveries"], [])


if __name__ == "__main__":
    unittest.main()
