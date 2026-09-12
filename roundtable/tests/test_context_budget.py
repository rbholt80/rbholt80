import tempfile
import unittest
from unittest.mock import patch

from roundtable import config, cli
from roundtable.config import Participant
from roundtable.engine import Roundtable, _system_prompt
from roundtable.providers import ProviderError


class ContextBudgetTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.small = Participant('Small', 'mock', context_tokens=2048, max_tokens=160)
        self.large = Participant('Large', 'mock', context_tokens=100000, max_tokens=160)
        self.table = Roundtable('Compare two proposals', [self.small, self.large], self.directory.name)

    def test_small_seat_trims_whole_turns_large_seat_sees_all(self):
        for i in range(40):
            self.table.add_host_message(f'Detail {i}: ' + 'quote information ' * 35)
        small = self.table._context(self.small)
        large = self.table._context(self.large)
        self.assertIn('earlier turn(s) omitted', small)
        self.assertIn('Detail 39:', small)
        self.assertNotIn('Detail 0:', small)
        self.assertIn('Detail 0:', large)
        self.assertIn('Detail 39:', large)
        system = _system_prompt(self.small, ['Large'], self.table.topic)
        self.assertLessEqual(self.table._estimate_tokens(system + small + '\n\nSmall:')
                             + self.small.max_tokens + 256, self.small.context_tokens)

    def test_newest_message_that_cannot_fit_records_error_without_calling_provider(self):
        self.table.add_host_message('x' * 20000)
        with patch('roundtable.engine.stream') as provider:
            events = list(self.table.run_turn(self.small))
        provider.assert_not_called()
        self.assertTrue(events[-1]['error'])
        self.assertIn('newest message exceed', events[-1]['text'])
        self.assertTrue(self.table.transcript_path.with_suffix('.md').exists())

    def test_full_system_topic_and_output_are_reserved(self):
        self.table.topic = 'long topic ' * 2000
        with self.assertRaises(ProviderError):
            self.table._context(self.small)

    def test_unicode_estimate_counts_utf8_not_only_characters(self):
        self.assertGreater(self.table._estimate_tokens('😀' * 100),
                           self.table._estimate_tokens('a' * 100))

    def test_independent_round_is_budgeted_from_frozen_turns(self):
        for i in range(15):
            self.table.add_host_message(f'Input {i}: ' + 'requirements ' * 50)
        self.table.queue_round(independent=True)
        prompts = {}
        def stream(seat, system, prompt, metrics):
            prompts[seat.name] = prompt
            if seat.context_tokens:
                self.assertLessEqual(self.table._estimate_tokens(system + prompt)
                    + seat.max_tokens + 256, seat.context_tokens)
            yield 'UNSEEN_PEER_ANSWER'
        with patch('roundtable.engine.stream', side_effect=stream):
            list(self.table.run_turn())
            list(self.table.run_turn())
        self.assertEqual(set(prompts), {'Small', 'Large'})
        self.assertIn('omitted', prompts['Small'])
        self.assertIn('Input 0:', prompts['Large'])
        self.assertNotIn('UNSEEN_PEER_ANSWER', prompts['Large'])

    def test_local_default_and_override_do_not_limit_hosted_seats(self):
        self.assertIsNone(Participant('Hosted', 'openai').context_tokens)
        self.assertEqual(config.LOCAL_CONTEXT_TOKENS, 2048)
        seats = [Participant('Local', 'ollama'), Participant('Hosted', 'openai')]
        with patch('roundtable.config.find_config', return_value=None), \
             patch('roundtable.config.discover', return_value=seats):
            resolved, _ = config.resolve(local_context=4096)
        self.assertEqual(resolved[0].context_tokens, 4096)
        self.assertIsNone(resolved[1].context_tokens)
        self.assertEqual(cli.build_parser().parse_args(['web', 'topic', '--local-context', '4096']).local_context, 4096)
        with self.assertRaises(ValueError):
            Participant('Invalid', 'ollama', context_tokens=0)


if __name__ == '__main__':
    unittest.main()
