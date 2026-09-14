"""A reviewer that jumps straight to accept without inspecting must get a
second chance, not be rotated away as if it had failed.

Observed live across two full goal-mode runs: every local reviewer seat
(six distinct small models across the two runs) either mis-formatted its
JSON or returned {"verdict":"accept"} on its very first turn, without
calling any inspection tool first. The old code raised that as a generic
error, which is caught by the same handler as a real tool failure: it
rotates to a different seat and burns a step. With a reviewer pool made
entirely of seats that behave this way, every review attempt in the whole
step budget failed the same way and none ever got a second try at the
two-step protocol.
"""
import tempfile
import unittest

import roundtable.work as work
import roundtable.worktools as worktools
from roundtable.config import Participant
from roundtable.work import WorkManager


class ReviewRetryTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.author = Participant('Author', 'mock')
        self.reviewer = Participant('Reviewer', 'mock')
        self.manager = WorkManager(self.directory.name, [self.author, self.reviewer])
        self.original_stream = work.stream
        self.original_fetch = worktools.WorkTools._fetch_url
        self.addCleanup(setattr, work, 'stream', self.original_stream)
        self.addCleanup(setattr, worktools.WorkTools, '_fetch_url', self.original_fetch)
        worktools.WorkTools._fetch_url = lambda self, url: {
            'url': url, 'status': 200, 'text': 'stub source content'}

    def _run_scripted(self, author_script, reviewer_script, max_steps=10):
        calls = {}

        def fake_stream(p, system, prompt, metrics=None):
            calls[p.name] = calls.get(p.name, 0) + 1
            script = author_script if p.name == 'Author' else reviewer_script
            return iter([script[calls[p.name] - 1]])

        work.stream = fake_stream
        identifier = self.manager.create('write a short report', mode='research',
                                         max_steps=max_steps)
        self.manager.start(identifier)
        self.manager.thread.join(timeout=10)
        return self.manager.get(identifier)

    def test_premature_accept_is_not_an_error_and_the_same_seat_retries(self):
        goal = self._run_scripted(
            author_script=[
                '{"tool":"write_file","path":"deliverable.md","text":"# Report\\n\\nfindings"}',
                '{"tool":"fetch_url","url":"https://example.com/source"}',
                # Evidence 2 and 4 are the write_file and fetch_url *tool*
                # result events; 1 and 3 are the matching request events.
                '{"tool":"finish","summary":"Delivered the report.","evidence":[2,4]}',
            ],
            reviewer_script=[
                '{"tool":"review","verdict":"accept","reason":"looks fine"}',
                '{"tool":"diff"}',
                '{"tool":"review","verdict":"accept","reason":"verified via diff"}',
            ])
        self.assertEqual(goal['status'], 'ready', goal['message'])
        self.assertEqual(goal['review']['seat'], 'Reviewer')
        # The premature accept must be recorded as a revise-style nudge, not
        # a hard failure -- and it must never appear as an 'error' event.
        kinds = [e['kind'] for e in goal['events']]
        self.assertNotIn('error', kinds)
        nudges = [e for e in goal['events'] if e['kind'] == 'review'
                  and 'without inspecting' in e['summary']]
        self.assertEqual(len(nudges), 1)

    def test_a_seat_that_never_inspects_eventually_exhausts_its_budget_safely(self):
        # Guard against the retry becoming an infinite loop: a reviewer that
        # always jumps straight to accept must still stop once the step
        # budget runs out, not spin forever.
        goal = self._run_scripted(
            author_script=[
                '{"tool":"write_file","path":"deliverable.md","text":"# Report\\n\\nfindings"}',
                '{"tool":"fetch_url","url":"https://example.com/source"}',
                '{"tool":"finish","summary":"Delivered the report.","evidence":[2,4]}',
            ],
            reviewer_script=['{"tool":"review","verdict":"accept","reason":"looks fine"}'] * 20,
            max_steps=6)
        self.assertEqual(goal['status'], 'budget')
        self.assertEqual(goal['steps'], goal['limit'])


class EmptyResponseTests(unittest.TestCase):
    """A model that produces zero output tokens must not surface as a raw,
    context-free JSONDecodeError.

    Observed live, repeatedly: six distinct small local models each
    returned an empty response at some point across two real goal-mode
    runs, and every one of them was reported as
    'Worker returned invalid JSON: Expecting value: line 1 column 1
    (char 0)' -- true, but it reads like a parser bug rather than what
    actually happened (no output at all, most likely a context-budget
    problem: the SYSTEM prompt alone is already ~2100 bytes, leaving a
    local seat's default budget very little room for the rest).
    """
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.seat = Participant('TinyLocal', 'mock', context_tokens=2048)
        self.manager = WorkManager(self.directory.name, [self.seat])
        self.original_stream = work.stream
        self.addCleanup(setattr, work, 'stream', self.original_stream)

    def test_empty_output_gets_a_specific_actionable_message(self):
        work.stream = lambda p, system, prompt, metrics=None: iter([''])
        identifier = self.manager.create('a task', mode='research', max_steps=5)
        goal = self.manager.get(identifier)
        with self.assertRaises(ValueError) as cm:
            self.manager._ask(goal, self.seat)
        message = str(cm.exception)
        self.assertIn('no output at all', message)
        self.assertNotIn('Expecting value', message)

    def test_whitespace_only_output_is_treated_the_same_as_empty(self):
        work.stream = lambda p, system, prompt, metrics=None: iter(['   \n  '])
        identifier = self.manager.create('a task', mode='research', max_steps=5)
        goal = self.manager.get(identifier)
        with self.assertRaises(ValueError) as cm:
            self.manager._ask(goal, self.seat)
        self.assertIn('no output at all', str(cm.exception))


if __name__ == '__main__':
    unittest.main()
