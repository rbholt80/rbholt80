"""Offline provider contracts: usage, dialect negotiation, and local discovery."""
import io
import json
import sys
import unittest
from dataclasses import replace
from types import SimpleNamespace
from unittest.mock import patch

from roundtable.config import Participant, _ollama_chat_models, discover
from roundtable.providers import ProviderError, _DIALECT, stream


class BadRequestError(Exception):
    pass


class FakeResponse:
    def __init__(self, chunks):
        self.chunks = chunks
        self.closed = False

    def __iter__(self):
        yield from self.chunks

    def close(self):
        self.closed = True


def chunk(text=None, usage=None):
    return SimpleNamespace(
        choices=[] if text is None else [SimpleNamespace(delta=SimpleNamespace(content=text))],
        usage=usage,
    )


class FakeOpenAI:
    def __init__(self, token_field='max_completion_tokens', include_usage=True):
        self.token_field = token_field
        self.include_usage = include_usage
        self.calls = []
        self.responses = []
        self.closed = 0
        self.chat = SimpleNamespace(completions=self)

    def create(self, **kwargs):
        self.calls.append(kwargs)
        other = 'max_tokens' if self.token_field == 'max_completion_tokens' else 'max_completion_tokens'
        if other in kwargs:
            raise BadRequestError(f'unsupported parameter {other}')
        if not self.include_usage and 'stream_options' in kwargs:
            raise BadRequestError('unsupported parameter stream_options')
        usage = SimpleNamespace(prompt_tokens=25, completion_tokens=6)
        response = FakeResponse([chunk('answer'), chunk(usage=usage if self.include_usage else None)])
        self.responses.append(response)
        return response

    def close(self):
        self.closed += 1

    def module(self):
        return SimpleNamespace(OpenAI=lambda **kwargs: self, BadRequestError=BadRequestError)


class ProviderIntegrationTests(unittest.TestCase):
    def setUp(self):
        _DIALECT.clear()

    def test_dialect_cache_preserves_current_budget_and_usage(self):
        for field, usage in [('max_completion_tokens', True), ('max_tokens', True),
                             ('max_completion_tokens', False), ('max_tokens', False)]:
            with self.subTest(field=field, usage=usage):
                _DIALECT.clear()
                fake = FakeOpenAI(field, usage)
                seat = Participant('A', 'openai', model='test', max_tokens=32)
                with patch.dict(sys.modules, {'openai': fake.module()}):
                    self.assertEqual(''.join(stream(seat, 'system', 'prompt')), 'answer')
                    count = len(fake.calls)
                    metrics = {}
                    self.assertEqual(''.join(stream(replace(seat, max_tokens=1024), 'system', 'prompt', metrics)), 'answer')
                self.assertEqual(len(fake.calls), count + 1)
                self.assertEqual(fake.calls[-1][field], 1024)
                self.assertEqual(metrics['output_tokens'], 6 if usage else 1)
                self.assertEqual(metrics.get('estimated', False), not usage)
                self.assertEqual(fake.closed, 2)
                self.assertTrue(all(response.closed for response in fake.responses))

    def test_unrelated_bad_request_does_not_negotiate(self):
        fake = FakeOpenAI()
        def invalid(**kwargs):
            fake.calls.append(kwargs)
            raise BadRequestError('model does not exist')
        fake.create = invalid
        with patch.dict(sys.modules, {'openai': fake.module()}):
            with self.assertRaisesRegex(ProviderError, 'model does not exist'):
                list(stream(Participant('A', 'openai'), '', ''))
        self.assertEqual(len(fake.calls), 1)
        self.assertEqual(fake.closed, 1)

    def test_openai_partial_failure_is_not_replayed(self):
        fake = FakeOpenAI()
        def failing_chunks():
            yield chunk('partial')
            raise RuntimeError('stream ended badly')
        response = FakeResponse(failing_chunks())
        def create(**kwargs):
            fake.calls.append(kwargs)
            return response
        fake.create = create
        metrics = {}
        received = []
        with patch.dict(sys.modules, {'openai': fake.module()}):
            with self.assertRaisesRegex(ProviderError, 'stream ended badly'):
                for text in stream(Participant('A', 'openai'), '', '', metrics):
                    received.append(text)
        self.assertEqual(received, ['partial'])
        self.assertEqual(len(fake.calls), 1)
        self.assertTrue(response.closed)
        self.assertTrue(metrics['estimated'])
        self.assertEqual(metrics['characters'], 7)

    def test_reported_zero_output_is_not_replaced_with_an_estimate(self):
        fake = FakeOpenAI()
        fake.create = lambda **kwargs: FakeResponse([
            chunk('cached answer'), chunk(usage=SimpleNamespace(prompt_tokens=10, completion_tokens=0))])
        metrics = {}
        with patch.dict(sys.modules, {'openai': fake.module()}):
            list(stream(Participant('A', 'openai'), '', '', metrics))
        self.assertEqual(metrics['output_tokens'], 0)
        self.assertFalse(metrics.get('estimated', False))

    def test_native_ollama_records_terminal_usage(self):
        events = [{'message': {'content': 'a reply'}, 'done': False},
                  {'done': True, 'prompt_eval_count': 43, 'eval_count': 9}]
        payload = '\n'.join(json.dumps(event) for event in events).encode()
        metrics = {}
        with patch('roundtable.local.urllib.request.urlopen', return_value=io.BytesIO(payload)) as opener:
            text = ''.join(stream(Participant('Local', 'ollama', model='small'), 'system', 'prompt', metrics))
        self.assertEqual(text, 'a reply')
        self.assertEqual(metrics['prompt_tokens'], 43)
        self.assertEqual(metrics['output_tokens'], 9)
        self.assertNotIn('estimated', metrics)
        request = opener.call_args.args[0]
        self.assertEqual(json.loads(request.data)['messages'][0]['content'], 'system')

    def test_native_ollama_missing_terminal_event_keeps_partial_reply(self):
        payload = json.dumps({'message': {'content': 'part'}}).encode()
        iterator = stream(Participant('Local', 'ollama'), '', '', {})
        with patch('roundtable.local.urllib.request.urlopen', return_value=io.BytesIO(payload)):
            self.assertEqual(next(iterator), 'part')
            with self.assertRaisesRegex(ProviderError, 'before the reply finished'):
                next(iterator)

    def test_cli_contract_accepts_metrics_and_estimates_only_output(self):
        metrics = {}
        seat = Participant('CLI', 'cli', model='test', argv=[sys.executable, '-c',
            "import sys; data=sys.stdin.read(); print('ready' if 'system' in data else 'bad')"])
        self.assertEqual(''.join(stream(seat, 'system', 'prompt', metrics)), 'ready\n')
        self.assertTrue(metrics['estimated'])
        self.assertNotIn('prompt_tokens', metrics)
        self.assertGreaterEqual(metrics['seconds'], 0)

    def test_native_discovery_keeps_roles_weights_and_unique_short_names(self):
        with patch.dict('os.environ', {}, clear=True), \
             patch('roundtable.config._port_open', side_effect=lambda port: port == 11434), \
             patch('roundtable.config._has_module', return_value=False), \
             patch('roundtable.config._ollama_chat_models', return_value=[('qwen:0.5b', .5), ('qwen:8b', 8)]):
            seats = discover(include_cli=False)
        self.assertEqual([seat.kind for seat in seats], ['ollama', 'ollama'])
        self.assertEqual([seat.name for seat in seats], ['Qwen', 'Qwen-8b'])
        self.assertEqual([seat.role for seat in seats], ['panel', 'principal'])
        self.assertEqual([seat.weight for seat in seats], [.35, 1.0])

    def test_discovery_uses_capabilities_and_legacy_embedding_fallback(self):
        entries = [{'name': 'chat:tiny', 'details': {'parameter_size': '596.05M'}},
                   {'name': 'nomic-embed-text'}, {'name': 'legacy-chat'}]
        def response(request, **kwargs):
            if isinstance(request, str):
                data = {'models': entries}
            else:
                model = json.loads(request.data)['model']
                data = {'capabilities': ['completion']} if model == 'chat:tiny' else {}
            return io.BytesIO(json.dumps(data).encode())
        with patch('roundtable.config.urllib.request.urlopen', side_effect=response):
            models = _ollama_chat_models('http://localhost:11434/v1')
        self.assertEqual(models, [('chat:tiny', .59605), ('legacy-chat', None)])

    def test_discovery_checks_login_and_retains_cli_restrictions(self):
        def which(exe):
            return '/tools/' + exe if exe in ('claude', 'codex') else None
        with patch.dict('os.environ', {}, clear=True), \
             patch('roundtable.config.shutil.which', side_effect=which), \
             patch('roundtable.config._has_module', return_value=False), \
             patch('roundtable.config.cli_ready', side_effect=lambda exe, path: (True, 'Signed in')):
            seats = discover(include_local=False)
        claude, codex = seats
        self.assertIn('--safe-mode', claude.argv)
        self.assertEqual(claude.argv[claude.argv.index('--tools') + 1], '')
        self.assertIn('--ignore-user-config', codex.argv)
        self.assertIn('--ignore-rules', codex.argv)
        self.assertEqual(codex.argv[codex.argv.index('--sandbox') + 1], 'read-only')
        self.assertIn('shell_tool', codex.argv)
        with patch.dict('os.environ', {}, clear=True), \
             patch('roundtable.config.shutil.which', side_effect=which), \
             patch('roundtable.config._has_module', return_value=False), \
             patch('roundtable.config.cli_ready', return_value=(False, 'Sign in')):
            self.assertEqual(discover(include_local=False), [])

    def test_anthropic_partial_type_error_is_not_replayed(self):
        calls = []
        class Response:
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            @property
            def text_stream(self):
                yield 'partial'
                raise TypeError('bad output_config decoding after partial reply')
        def create(**kwargs):
            calls.append(kwargs)
            return Response()
        fake = SimpleNamespace(messages=SimpleNamespace(stream=create), close=lambda: None)
        with patch.dict(sys.modules, {'anthropic': SimpleNamespace(Anthropic=lambda **kwargs: fake)}):
            reply = stream(Participant('A', 'anthropic', effort='low'), '', '')
            self.assertEqual(next(reply), 'partial')
            with self.assertRaisesRegex(ProviderError, 'TypeError'):
                next(reply)
        self.assertEqual(len(calls), 1)

    def test_anthropic_old_sdk_keyword_negotiation_before_text(self):
        calls = []
        class Response:
            text_stream = ['ok']
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def get_final_message(self):
                return SimpleNamespace(usage=SimpleNamespace(input_tokens=8, output_tokens=1))
        def create(**kwargs):
            calls.append(kwargs)
            if 'output_config' in kwargs:
                raise TypeError("unexpected keyword argument 'output_config'")
            return Response()
        fake = SimpleNamespace(messages=SimpleNamespace(stream=create), close=lambda: None)
        metrics = {}
        with patch.dict(sys.modules, {'anthropic': SimpleNamespace(Anthropic=lambda **kwargs: fake)}):
            self.assertEqual(''.join(stream(Participant('A', 'anthropic', effort='low'), '', '', metrics)), 'ok')
        self.assertEqual(len(calls), 2)
        self.assertEqual(metrics['prompt_tokens'], 8)
        self.assertEqual(metrics['output_tokens'], 1)

    def test_installed_openai_sdk_streaming_with_offline_http_transport(self):
        try:
            import httpx
            import openai
        except ImportError:
            self.skipTest('Optional OpenAI SDK is not installed')
        calls = []
        def handle(request):
            payload = json.loads(request.content)
            calls.append(payload)
            if 'max_completion_tokens' in payload:
                return httpx.Response(400, json={'error': {
                    'message': 'unsupported parameter max_completion_tokens',
                    'type': 'invalid_request_error', 'param': 'max_completion_tokens',
                }})
            events = [
                {'id': 'offline', 'object': 'chat.completion.chunk', 'created': 1,
                 'model': 'test', 'choices': [{'index': 0, 'delta': {'content': 'ok'},
                                              'finish_reason': None}]},
                {'id': 'offline', 'object': 'chat.completion.chunk', 'created': 1,
                 'model': 'test', 'choices': [],
                 'usage': {'prompt_tokens': 12, 'completion_tokens': 2, 'total_tokens': 14}},
            ]
            data = ''.join('data: ' + json.dumps(event) + '\n\n' for event in events)
            return httpx.Response(200, text=data + 'data: [DONE]\n\n',
                                  headers={'Content-Type': 'text/event-stream'})
        real_client = openai.OpenAI
        def client(**kwargs):
            return real_client(**kwargs, max_retries=0,
                               http_client=httpx.Client(transport=httpx.MockTransport(handle)))
        seat = Participant('A', 'openai', model='test', base_url='http://offline.invalid/v1', max_tokens=32)
        metrics = {}
        with patch.object(openai, 'OpenAI', side_effect=client):
            self.assertEqual(''.join(stream(seat, '', 'probe')), 'ok')
            self.assertEqual(''.join(stream(replace(seat, max_tokens=256), '', 'turn', metrics)), 'ok')
        self.assertEqual(len(calls), 3)
        self.assertEqual(calls[-1]['max_tokens'], 256)
        self.assertEqual(metrics['prompt_tokens'], 12)
        self.assertEqual(metrics['output_tokens'], 2)


if __name__ == '__main__':
    unittest.main()
