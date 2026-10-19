import json
import os
import subprocess
import time
import unittest
from urllib.error import URLError
from urllib.request import urlopen


@unittest.skipUnless(os.environ.get('YOINK_TEST_RELEASE_VERSION'), 'release image check only')
class ReleaseVersionTests(unittest.TestCase):
    def test_http_version_matches_the_image_release_without_recompiling_cargo_packages(self):
        expected = os.environ['YOINK_TEST_RELEASE_VERSION']
        process = subprocess.Popen(
            ['/usr/local/bin/yoink-server'],
            env={**os.environ, 'YOINK_RUST_HTTP_BIND': '127.0.0.1:18088'},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.monotonic() + 10
            while process.poll() is None and time.monotonic() < deadline:
                try:
                    with urlopen('http://127.0.0.1:18088/api/v1/system', timeout=0.2) as response:
                        snapshot = json.load(response)
                    self.assertEqual(snapshot['version'], expected)
                    return
                except URLError:
                    time.sleep(0.05)
            self.fail(f'Offline server did not become ready: exit={process.poll()}')
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
