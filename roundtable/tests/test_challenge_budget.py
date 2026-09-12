"""A challenge must never permanently lock a seat out of the conversation.

Observed live: a single ~5,200-character challenge (well within the 4,000
character limit on each of its two fields) put every 2048-token local seat
into a state where it failed identically on every subsequent turn -- 80+
consecutive turns of "[Name unavailable: ... exceeds ... token context
budget ...]" with no recovery, because that challenge became the pinned
"latest Host message" every seat's turn is built around (see
Roundtable._context()) until a newer one replaced it, and nothing ever did.
"""

import tempfile
import unittest

from roundtable.config import Participant
from roundtable.engine import Roundtable, Turn
from roundtable.providers import ProviderError

QUOTE_FILLER = "This is the quoted claim from a prior reply. "
QUESTION_FILLER = "Assess whether this holds under the following conditions. "


class ChallengeBudgetTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.large = Participant('Claude', 'mock', context_tokens=None)
        self.small = Participant('Gemma3', 'mock', role='panel', weight=0.35,
                                 context_tokens=2048, max_tokens=160)
        self.table = Roundtable('Should Joey teach vim or nano first?',
                                [self.large, self.small], self.directory.name)

    def _seed(self, text: str) -> Turn:
        turn = Turn(speaker='Claude', text=text, seq=self.table._allocate_seq())
        self.table.history.append(turn)
        return turn

    def test_the_exact_live_failure_is_now_rejected_at_creation_not_deferred(self):
        quote = (QUOTE_FILLER * 60).strip()
        question = (QUESTION_FILLER * 40).strip()
        seed = self._seed(quote)
        with self.assertRaises(ValueError) as caught:
            self.table.add_challenge(seed.seq, quote, question)
        self.assertIn('too long', str(caught.exception))
        # And critically: nothing was created, so there is no lingering turn
        # for every future rotation to keep failing against.
        self.assertEqual(len(self.table.history), 1)

    def test_oversized_question_is_truncated_and_the_quote_stays_exact(self):
        quote = (QUOTE_FILLER * 15).strip()
        question = (QUESTION_FILLER * 50).strip()
        seed = self._seed(quote)
        challenge = self.table.add_challenge(seed.seq, quote, question)
        self.assertEqual(challenge.reference['quote'], quote)
        self.assertIn(quote, challenge.text)
        self.assertIn('[...truncated for length]', challenge.text)
        # The point of the fix: it must still fit, repeatedly, not just once.
        for _ in range(3):
            self.table._context(self.small)

    def test_a_small_ordinary_challenge_is_untouched(self):
        quote = 'costs 50 dollars'
        seed = self._seed(f'Plan A {quote} per month.')
        challenge = self.table.add_challenge(seed.seq, quote, 'Does this include support?')
        self.assertNotIn('[...truncated for length]', challenge.text)
        self.assertIn('Does this include support?', challenge.text)
        ctx = self.table._context(self.small)
        self.assertIn('Does this include support?', ctx)

    def test_no_local_seats_configured_means_no_cap_at_all(self):
        table = Roundtable('x', [self.large], self.directory.name)
        quote = (QUOTE_FILLER * 60).strip()
        question = (QUESTION_FILLER * 40).strip()
        seed = Turn(speaker='Claude', text=quote, seq=table._allocate_seq())
        table.history.append(seed)
        challenge = table.add_challenge(seed.seq, quote, question)
        self.assertIn(question, challenge.text)

    def test_repeated_challenges_never_leave_a_seat_permanently_stuck(self):
        # A fuzz-style sweep rather than one fixed size: the fix should hold
        # across the whole range the existing 1-4000 character validation
        # allows, not just the one size this bug was first found at.
        for quote_mult in (1, 5, 15, 30, 50, 64):
            for question_mult in (1, 10, 30, 50, 64):
                with self.subTest(quote_mult=quote_mult, question_mult=question_mult):
                    directory = tempfile.TemporaryDirectory()
                    self.addCleanup(directory.cleanup)
                    table = Roundtable('x', [
                        Participant('Claude', 'mock', context_tokens=None),
                        Participant('Gemma3', 'mock', role='panel',
                                   context_tokens=2048, max_tokens=160),
                    ], directory.name)
                    quote = (QUOTE_FILLER * quote_mult).strip()
                    question = (QUESTION_FILLER * question_mult).strip()
                    seed = Turn(speaker='Claude', text=quote, seq=table._allocate_seq())
                    table.history.append(seed)
                    try:
                        table.add_challenge(seed.seq, quote, question)
                    except ValueError:
                        continue  # rejected cleanly at creation -- acceptable
                    # If it was created, it must actually be usable, more
                    # than once, not just on the first attempt.
                    for _ in range(3):
                        table._context(table.participants[1])


if __name__ == '__main__':
    unittest.main()
