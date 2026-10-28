"""Pinned source downloads, without network access or compilation."""

import hashlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "prepare_tmux", Path(__file__).resolve().parents[1] / "prepare-tmux.py")
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class DownloadTests(unittest.TestCase):
    def test_cache_miss_publishes_only_verified_source(self):
        data = b"source archive fixture"
        item = {"name": "source.tar.gz", "url": "https://example.invalid/source",
                "sha256": hashlib.sha256(data).hexdigest()}
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            with patch.object(builder.urllib.request, "urlopen", return_value=io.BytesIO(data)) as fetch:
                archive = builder.fetch(item, cache)
                fetch.assert_called_once_with(item["url"], timeout=120)
            self.assertEqual(archive.read_bytes(), data)
            with patch.object(builder.urllib.request, "urlopen") as fetch:
                self.assertEqual(builder.fetch(item, cache), archive)
                fetch.assert_not_called()
            self.assertEqual(list(cache.iterdir()), [archive])

    def test_bad_download_leaves_no_cached_or_temporary_file(self):
        item = {"name": "source.tar.gz", "url": "https://example.invalid/source",
                "sha256": "0" * 64}
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            with patch.object(builder.urllib.request, "urlopen", return_value=io.BytesIO(b"bad")):
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    builder.fetch(item, cache)
            self.assertEqual(list(cache.iterdir()), [])
