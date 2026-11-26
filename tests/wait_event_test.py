"""Standalone waiter regression tests: python3 -m unittest discover -s tests -p wait_event_test.py."""
import os
import pathlib
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "skills/discuss/wait-event.sh"
URL = "http://127.0.0.1:1/api/state"


class WaitEventTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.events = pathlib.Path(self.tmp.name) / "events.jsonl"
        self.events.write_bytes(b"")

    def waiter(self, handled=0, env=None):
        proc = subprocess.Popen(
            ["bash", str(SCRIPT), str(self.events), str(handled), URL],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
        )
        self.addCleanup(lambda: proc.poll() is None and (proc.kill(), proc.communicate()))
        return proc

    def result(self, proc):
        out, err = proc.communicate(timeout=8)
        self.assertEqual(proc.returncode, 0, err)
        return out.splitlines()

    def test_partial_append_is_never_printed_or_advanced(self):
        with self.events.open("ab") as f:
            f.write(b'{"kind":"thread.created","payload":')
        proc = self.waiter()
        with self.assertRaises(subprocess.TimeoutExpired):
            proc.communicate(timeout=0.2)
        with self.events.open("ab") as f:
            f.write(b'{} }\n')
        self.assertEqual(self.result(proc), ["LINES=1", '{"kind":"thread.created","payload":{} }'])
        # A completed event is not replayed from the returned cursor.
        with self.events.open("ab") as f:
            f.write(b'{"kind":"reply.added"}\n')
        self.assertEqual(self.result(self.waiter(1)), ["LINES=2", '{"kind":"reply.added"}'])

    def test_prompt_filter_advances_cursor_but_preserves_pr_and_source(self):
        self.events.write_text(
            '{"kind":"session.started"}\n'
            '{"kind":"prompt.suggest_done"}\n'
            '{"kind":"pr.imported"}\n'
            '{"kind":"pr.summary.requested"}\n'
            '{"kind":"source.updated"}\n'
        )
        self.assertEqual(self.result(self.waiter(1)), [
            "LINES=5", '{"kind":"pr.imported"}',
            '{"kind":"pr.summary.requested"}', '{"kind":"source.updated"}',
        ])

    def test_only_prompt_and_partial_line_remain_pending(self):
        self.events.write_bytes(b'{"kind":"prompt.suggest_done"}\n{"kind":"reply.added"')
        proc = self.waiter()
        with self.assertRaises(subprocess.TimeoutExpired):
            proc.communicate(timeout=0.2)
        with self.events.open("ab") as f:
            f.write(b'}\n')
        self.assertEqual(self.result(proc), ["LINES=2", '{"kind":"reply.added"}'])

    def test_failed_bounded_curl_eventually_reports_done(self):
        tools = pathlib.Path(self.tmp.name) / "bin"
        tools.mkdir()
        calls = pathlib.Path(self.tmp.name) / "curl-args"
        (tools / "curl").write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$CALLS"\nexit 28\n')
        (tools / "sleep").write_text('#!/bin/sh\nexit 0\n')
        for tool in tools.iterdir():
            tool.chmod(0o755)
        env = dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}', CALLS=str(calls))
        self.assertEqual(self.result(self.waiter(env=env)), ['{"event": "session.done"}'])
        self.assertEqual(calls.read_text().splitlines(),
                         [f'-s -o /dev/null -f --max-time 5 {URL}'] * 3)


if __name__ == "__main__":
    unittest.main()
