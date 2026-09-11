import json
import tempfile
import unittest
from unittest.mock import patch

from roundtable.config import Participant
from roundtable.engine import Roundtable


class DiscussionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.seats = [Participant('A', 'mock', weight=1),
                      Participant('B', 'mock', weight=.35),
                      Participant('C', 'mock', weight=.35)]
        self.table = Roundtable('Choose a plan', self.seats, self.directory.name)

    def test_independent_round_sees_same_baseline_without_peer_answers(self):
        calls = []
        def reply(seat, system, prompt, metrics):
            calls.append((seat.name, system, prompt))
            yield f'{seat.name} unique answer'
        self.table.add_host_message('Budget is 100 dollars')
        self.table.queue_round(independent=True)
        with patch('roundtable.engine.stream', side_effect=reply):
            for _ in self.seats:
                list(self.table.run_turn())
        self.assertEqual({name for name, _, _ in calls}, {'A', 'B', 'C'})
        for _, system, prompt in calls:
            self.assertIn('independent round', system)
            self.assertIn('Budget is 100 dollars', prompt)
            self.assertNotIn('unique answer', prompt)
        self.assertEqual(self.table.pending_round, 0)

    def test_independent_round_honors_fresh_host_input(self):
        self.table.queue_round(independent=True)
        with patch('roundtable.engine.stream', return_value=iter(['Peer answer'])):
            list(self.table.run_turn())
        self.table.add_host_message('The budget changed to 200 dollars')
        calls = []
        def reply(seat, system, prompt, metrics):
            calls.append(prompt)
            yield 'answer'
        with patch('roundtable.engine.stream', side_effect=reply):
            list(self.table.run_turn())
        self.assertIn('200 dollars', calls[0])
        self.assertNotIn('Peer answer', calls[0])

    def test_fair_round_and_forced_seat_do_not_duplicate(self):
        self.table.queue_round()
        self.table.force_next('C')
        with patch('roundtable.engine.stream', side_effect=lambda *args: iter(['answer'])):
            for _ in self.seats:
                list(self.table.run_turn())
        self.assertEqual([t.speaker for t in self.table.history], ['C', 'A', 'B'])
        self.assertEqual(self.table.pending_round, 0)
        self.table.queue_round(independent=True)
        self.table.clear_round()
        self.assertEqual(self.table.pending_round, 0)
        self.assertFalse(self.table._independent)

    def test_inflight_interjection_has_unique_ids_and_context_provenance(self):
        first = self.table.add_host_message('First question')
        with patch('roundtable.engine.stream', return_value=iter(['Partial', ' reply'])):
            events = self.table.run_turn()
            start = next(events)
            host = self.table.add_host_message('New fact while model was answering')
            end = list(events)[-1]
        self.assertEqual(start['seq'], end['seq'])
        self.assertNotEqual(host.seq, end['seq'])
        self.assertEqual(end['responding_to_seq'], first.seq)
        self.assertEqual(len({t.seq for t in self.table.history}), 3)
        saved = [json.loads(line) for line in self.table.transcript_path.read_text().splitlines()]
        self.assertEqual(saved[-1]['responding_to_seq'], first.seq)

    def test_challenge_preserves_exact_source_when_window_omits_original(self):
        with patch('roundtable.engine.stream', return_value=iter(['Plan A costs 50 dollars.'])):
            list(self.table.run_turn())
        source = self.table.history[0]
        self.table.context_turns = 1
        challenge = self.table.add_challenge(source.seq, 'costs 50 dollars', 'Does this include support?')
        self.assertEqual(challenge.reference['seq'], source.seq)
        self.assertEqual(challenge.reference['quote'], 'costs 50 dollars')
        self.assertIn('costs 50 dollars', self.table._context())
        self.assertIn('Does this include support?', self.table._context())
        self.assertIn('Agreement is allowed', challenge.text)
        with self.assertRaises(ValueError):
            self.table.add_challenge(source.seq, 'costs 500 dollars')
        with self.assertRaises(ValueError):
            self.table.add_challenge(challenge.seq, 'include support')
        with self.assertRaises(ValueError):
            self.table.add_challenge(True, 'costs 50 dollars')


if __name__ == '__main__':
    unittest.main()
