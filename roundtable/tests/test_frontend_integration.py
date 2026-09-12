import contextlib
import io
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from roundtable import cli
from roundtable.config import Participant
from roundtable.engine import Roundtable
from roundtable.web import Hub


class FrontendIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.table = Roundtable('test', [Participant('A', 'mock'), Participant('B', 'mock')], self.directory.name)
        with patch('roundtable.web.blocked', return_value=[]):
            self.hub = Hub(self.table, 'test-token')
        self.addCleanup(self.hub.stopping.set)

    def test_snapshot_between_persistence_and_broadcast_has_no_duplicate(self):
        turn = self.table.add_host_message('persisted before broadcast')
        q, snapshot = self.hub.subscribe_snapshot()
        self.assertFalse(snapshot['history'])
        self.hub.broadcast(turn.as_event())
        self.assertEqual(q.get_nowait()['seq'], turn.seq)
        self.assertTrue(q.empty())
        later, snapshot = self.hub.subscribe_snapshot()
        self.assertEqual([t['seq'] for t in snapshot['history']], [turn.seq])
        self.assertTrue(later.empty())

    def test_snapshot_during_model_finish_preserves_partial_then_one_end(self):
        with patch('roundtable.engine.stream', return_value=iter(['first ', 'last'])):
            events = self.table.run_turn()
            for _ in range(3):
                self.hub.broadcast(next(events))
            end = next(events)  # engine has saved it; no end broadcast yet
            q, snapshot = self.hub.subscribe_snapshot()
            self.assertFalse(snapshot['history'])
            self.assertEqual(snapshot['active']['text'], 'first last')
            self.hub.broadcast(end)
        self.assertEqual(q.get_nowait()['seq'], snapshot['active']['seq'])
        self.assertTrue(q.empty())
        self.assertIsNone(self.hub.snapshot()['active'])
        self.assertEqual(len(self.hub.snapshot()['history']), 1)

    def test_500_subscribers_racing_writer_get_each_host_turn_once(self):
        started = threading.Event()
        def write():
            started.set()
            for i in range(80):
                self.hub.say(f'message {i}')
                time.sleep(0)
        worker = threading.Thread(target=write)
        worker.start()
        started.wait(2)
        clients = [self.hub.subscribe_snapshot() for _ in range(500)]
        worker.join(10)
        self.assertFalse(worker.is_alive())
        for q, snapshot in clients:
            ids = [t['seq'] for t in snapshot['history']]
            while not q.empty():
                ids.append(q.get_nowait()['seq'])
            self.assertEqual(len(ids), 80)
            self.assertEqual(len(set(ids)), 80)
            self.hub.unsubscribe(q)

    def test_web_independent_round_stops_after_one_each(self):
        prompts = []
        def reply(seat, system, prompt, metrics):
            prompts.append(prompt)
            yield seat.name + ' reply marker'
        self.hub.pause = 0
        with patch('roundtable.engine.stream', side_effect=reply):
            self.assertTrue(self.hub.run_round(independent=True))
            deadline = time.monotonic() + 3
            while self.hub.running and time.monotonic() < deadline:
                time.sleep(.01)
        self.assertFalse(self.hub.running)
        self.assertEqual({t.speaker for t in self.table.history}, {'A', 'B'})
        self.assertEqual(len(prompts), 2)
        self.assertTrue(all('reply marker' not in p for p in prompts))
        self.hub.challenge(self.table.history[0].seq, 'reply marker', 'What supports that?')
        self.assertEqual(self.hub.snapshot()['history'][-1]['reference']['quote'], 'reply marker')

    def test_cli_opening_challenge_and_cost_share_engine(self):
        args = cli.build_parser().parse_args(['talk', 'test', '--pause', '0'])
        with patch('roundtable.cli._build', return_value=(self.table, {})), \
             patch('builtins.input', side_effect=['/opening', '/challenge 0 | reply marker | Evidence?', '/cost', '/quit']), \
             patch('roundtable.engine.stream', side_effect=lambda *args: iter(['reply marker'])), \
             contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(cli.cmd_talk(args), 0)
        self.assertEqual([t.speaker for t in self.table.history], ['A', 'B', 'Host', 'A'])
        self.assertIn('Evidence?', self.table.history[2].text)
        self.assertIn('seat', output.getvalue())

    def test_new_topic_preserves_explicit_roster_refresh(self):
        self.table.refresh_participants = lambda: [self.table.participants[0]]
        with patch('roundtable.web.discover', side_effect=AssertionError('must not broaden roster')), \
             patch('roundtable.web.blocked', return_value=[]):
            self.assertTrue(self.hub.new_topic('another local topic'))
        self.assertEqual(self.hub.table.names, ['A'])


if __name__ == '__main__':
    unittest.main()
