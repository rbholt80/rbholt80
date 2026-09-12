"""A seat that just failed should not be re-selected on every rotation.

Observed live: Codex-CLI failed on a login error identically on every single
automatic pick for 80+ consecutive turns -- each one a real subprocess spawn
and timeout that was always going to fail the same way. The rotation had no
memory of a seat's last outcome.
"""

import tempfile
import unittest

from roundtable.config import Participant
from roundtable.engine import Roundtable
from roundtable.providers import ProviderError


class SeatBackoffTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.claude = Participant('Claude', 'mock')
        self.broken = Participant('Codex-CLI', 'mock')
        self.gemma = Participant('Gemma3', 'mock', role='panel', weight=0.35)
        self.table = Roundtable('x', [self.claude, self.broken, self.gemma],
                                self.directory.name, policy='round_robin')

    def _always_fails(self, name):
        def fake(p, system, prompt, metrics=None):
            if p.name == name:
                raise ProviderError('exited 1; check its login/status')
            yield f'a real reply from {p.name}'
        return fake

    def test_a_persistently_broken_seat_is_picked_far_less_often(self):
        import roundtable.engine as eng
        eng.stream = self._always_fails('Codex-CLI')
        for _ in range(90):
            list(self.table.run_turn())
        order = [t.speaker for t in self.table.history]
        # Naive round-robin over three seats picks each ~30 times in 90 turns.
        self.assertLess(order.count('Codex-CLI'), 10)

    def test_a_broken_seat_is_never_permanently_excluded(self):
        import roundtable.engine as eng
        eng.stream = self._always_fails('Codex-CLI')
        for _ in range(90):
            list(self.table.run_turn())
        self.assertIn('Codex-CLI', [p.name for p in self.table.regulars])
        # And once fixed, it gets picked again promptly -- not stuck forever.
        eng.stream = lambda p, s, pr, metrics=None: iter([f'ok from {p.name}'])
        mark = len(self.table.history)
        for _ in range(15):
            list(self.table.run_turn())
        self.assertGreaterEqual(
            [t.speaker for t in self.table.history[mark:]].count('Codex-CLI'), 2)

    def test_round_robin_never_repeats_the_same_speaker_back_to_back(self):
        # Regression for the bug the backoff fix itself introduced: an index
        # tracked against the unfiltered roster, applied to a pool a
        # cooldown had shrunk, converged on a fixed point and repeated one
        # speaker indefinitely once a seat started cooling down.
        import roundtable.engine as eng
        eng.stream = self._always_fails('Codex-CLI')
        for _ in range(40):
            list(self.table.run_turn())
        order = [t.speaker for t in self.table.history]
        for i in range(len(order) - 1):
            self.assertNotEqual(order[i], order[i + 1],
                                f'{order[i]!r} repeated back to back at index {i}')

    def test_one_off_failure_costs_one_skipped_turn_not_a_long_ban(self):
        import roundtable.engine as eng
        calls = {'n': 0}
        def flaky(p, system, prompt, metrics=None):
            calls['n'] += 1
            if p.name == 'Codex-CLI' and calls['n'] == 2:
                raise ProviderError('a one-off network blip')
            yield f'ok from {p.name}'
        eng.stream = flaky
        for _ in range(6):
            list(self.table.run_turn())
        order = [t.speaker for t in self.table.history]
        # A single failure should not suppress it for long -- it should
        # reappear within the very next couple of rotations, not vanish for
        # the length of the whole test.
        self.assertIn('Codex-CLI', order[2:])


if __name__ == '__main__':
    unittest.main()
