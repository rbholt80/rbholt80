"""A seat pool where most seats fail almost every turn must not force the
one reliable seat to wait behind them at a constant, unchanging rate.

Observed live: with 8 seats in rotation and most of them structurally
unable to produce a valid action, the one capable seat (Claude-CLI) kept
waiting through the whole rest of the roster's failures every single time
its own turn also happened to fail -- goal mode's rotation had no memory
of a seat's recent outcome, unlike discussion mode's engine.py, which
already solved exactly this problem (see tests/test_seat_backoff.py) with
an exponential cooldown. This ports that same proven mechanism.

Goal mode's rotation is not simple round-robin, though, and that is
deliberate, not a bug this file is fixing: a seat that keeps succeeding
keeps being reselected on purpose (goal mode is one continuous task, not
a fairness-constrained discussion), confirmed by tests in
test_work_review.py that depend on exactly this -- an author completing
several actions in a row before any handoff. So these tests check the
cooldown's actual effect (a chronically-failing seat's retries thin out;
the loop never stalls even when every seat is simultaneously cooling
down) using an 8-seat roster matching the real one this was found on --
WorkManager.seats() sorts `cli`-kind seats first regardless of
construction order, so a real setup already tends to favor Claude-CLI's
position; a smaller roster hits the unrelated pre-existing
errors >= max(3, len(seats)) safety bailout almost immediately regardless
of cooldown, which isn't what these tests are checking.
"""
import tempfile
import unittest

import roundtable.work as work
from roundtable.config import Participant
from roundtable.work import WorkManager


class GoalSeatBackoffTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        # kind='cli' for Reliable, matching Claude-CLI's real kind, which
        # seats() sorts before any 'ollama' seat regardless of
        # construction order -- listed last here specifically to prove
        # that construction order isn't what's doing the work.
        self.chronic = [Participant(f'Chronic{i}', 'ollama') for i in range(7)]
        self.reliable = Participant('Reliable', 'cli')
        self.manager = WorkManager(self.directory.name, self.chronic + [self.reliable])
        self.original_stream = work.stream
        self.addCleanup(setattr, work, 'stream', self.original_stream)

    def test_a_seat_that_also_fails_sometimes_still_dominates_a_chronic_pool(self):
        # Reliable fails 1 in 4 calls (a real formatting slip, not always
        # successful) -- the exact live shape of the problem, not the
        # trivial case of a seat that literally never fails.
        calls = {}
        count = {'Reliable': 0}

        def fake_stream(p, system, prompt, metrics=None):
            calls[p.name] = calls.get(p.name, 0) + 1
            if p.name == 'Reliable':
                count['Reliable'] += 1
                if count['Reliable'] % 4 == 0:
                    return iter([''])
                return iter(['{"tool":"list_files"}'])
            return iter([''])  # every Chronic seat always fails
        work.stream = fake_stream

        identifier = self.manager.create('a task', mode='research', max_steps=40)
        self.manager.start(identifier)
        self.manager.thread.join(timeout=10)
        goal = self.manager.get(identifier)

        # Must run its full budget, not bail into needs_input just because
        # 7 of the 8 seats never once produce a valid action.
        self.assertEqual(goal['status'], 'budget', goal['message'])
        self.assertEqual(goal['steps'], 40)
        # The core property: Reliable gets the large majority of turns
        # despite its own real failure rate, not a naive ~1/8 share and
        # not smothered by seven chronically-broken seats.
        chronic_total = sum(n for name, n in calls.items() if name != 'Reliable')
        self.assertGreater(calls['Reliable'], chronic_total)

    def test_the_loop_never_stalls_even_when_every_seat_is_simultaneously_cooling_down(self):
        # All eight seats fail every single time. off_cooldown's own
        # contract (never return an empty pool) must hold even once every
        # seat in the roster has an active cooldown at the same moment --
        # confirmed by the run making real forward progress (one recorded
        # attempt per step) up to the point it correctly gives up, rather
        # than hanging or crashing.
        work.stream = lambda p, system, prompt, metrics=None: iter([''])
        identifier = self.manager.create('a task', mode='research', max_steps=20)
        self.manager.start(identifier)
        self.manager.thread.join(timeout=10)
        goal = self.manager.get(identifier)
        # With every seat always failing, the pre-existing
        # errors >= max(3, len(seats)) bail-out fires before the step
        # budget does -- this is that safety net working as designed, not
        # something cooldown should try to prevent.
        self.assertEqual(goal['status'], 'needs_input', goal['message'])
        self.assertLess(goal['steps'], 20)
        self.assertGreaterEqual(goal['steps'], 8)


if __name__ == '__main__':
    unittest.main()
