"""Live binary checks, shared by Linux/macOS CI. No third-party Python modules."""
import json
import math
import os
import subprocess
import unittest

BIN = os.environ.get("TMUX_STATUS_BIN", "target/release/tmux-status")


class Smoke(unittest.TestCase):
    def test_once_is_explicit_about_missing_cpu_baseline(self):
        data = json.loads(subprocess.check_output([BIN, "--once", "--json"], timeout=10))
        self.assertIsNone(data["cpu_percent"])
        self.assertIsNone(data["reachability"])
        self.assertGreater(data["memory"]["total_bytes"], 0)
        self.assertLessEqual(data["memory"]["used_bytes"], data["memory"]["total_bytes"])
        if data["battery"] is not None:
            self.assertTrue(0 <= data["battery"]["percent"] <= 100)

    def test_stream_has_interval_cpu_and_exits_when_pipe_closes(self):
        process = subprocess.Popen([BIN, "--json", "--interval", "1", "--no-battery"], stdout=subprocess.PIPE)
        try:
            first = json.loads(process.stdout.readline())
            second = json.loads(process.stdout.readline())
            self.assertIsNone(first["cpu_percent"])
            self.assertTrue(math.isfinite(second["cpu_percent"]))
            self.assertTrue(0 <= second["cpu_percent"] <= 100)
            process.stdout.close()
            self.assertEqual(process.wait(timeout=2), 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_idle_long_interval_process_exits_promptly_on_pipe_close(self):
        process = subprocess.Popen([BIN, "--json", "--interval", "3600", "--no-battery"], stdout=subprocess.PIPE)
        try:
            json.loads(process.stdout.readline())
            process.stdout.close()
            self.assertEqual(process.wait(timeout=2), 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_partial_style_defaults_text_mode_and_invalid_config(self):
        output = subprocess.check_output([BIN, '--once', '--no-date', '--no-battery'], text=True, timeout=5)
        self.assertIn(' ', output); self.assertIn(' ', output)
        output = subprocess.check_output([BIN, '--once', '--labels', 'text', '--no-date', '--no-battery'], text=True, timeout=5)
        self.assertIn('CPU:', output); self.assertIn('MEM:', output)
        output = subprocess.check_output([BIN, '--once', '--cpu-label', 'C: ', '--no-date', '--no-battery'], text=True, timeout=5)
        self.assertIn('C: ', output); self.assertIn(' ', output)
        for style in ['{"cpu_high":101}', '{"cpu_labl":"typo"}', '{"mem_template":"{typo}"}']:
            result = subprocess.run([BIN, '--check-config', '--appearance', style], capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 2)

    def test_invalid_intervals_and_hostname_probe_are_rejected(self):
        for args in [["--interval", "0"], ["--probe", "example.com:443"], ["--probe-timeout-ms", "0"]]:
            result = subprocess.run([BIN, *args], capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
