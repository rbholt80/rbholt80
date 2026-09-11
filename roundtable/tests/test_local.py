import json
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from roundtable.config import Participant, discover
from roundtable.engine import Roundtable
from roundtable.local import stream_cli
from roundtable.web import Hub, _handler_factory


class LocalTests(unittest.TestCase):
    def test_cli_drains_large_stderr_and_stdin(self):
        code = "import sys; sys.stderr.write('e'*1000000); data=sys.stdin.read(); print('héllo',len(data))"
        p = Participant('Test', 'cli', model='test', argv=[sys.executable, '-c', code], timeout=5)
        text = ''.join(stream_cli(p, 'system', 'x'*1000000))
        self.assertIn('héllo 1000013', text)

    def test_cli_timeout_kills_descendant_holding_pipes(self):
        code = "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)']); time.sleep(30)"
        p = Participant('Test', 'cli', model='test', argv=[sys.executable, '-c', code], timeout=.2)
        started = time.monotonic()
        with self.assertRaisesRegex(RuntimeError, 'timed out'):
            list(stream_cli(p, '', ''))
        self.assertLess(time.monotonic()-started, 3)

    def test_failure_after_partial_reply_marks_turn_as_error(self):
        def broken(*args):
            yield 'partial reply'
            raise RuntimeError('broken connection')
        with tempfile.TemporaryDirectory() as directory:
            table = Roundtable('test', [Participant('A','mock')], directory)
            with patch('roundtable.providers._ADAPTERS', {'mock': broken}):
                events = list(table.run_turn())
            self.assertTrue(events[-1]['error'])
            self.assertIn('partial reply', table.history[-1].text)
            self.assertTrue(table.transcript_path.with_suffix('.md').exists())

    def test_transcript_names_and_mentions(self):
        with tempfile.TemporaryDirectory() as directory:
            seats = [Participant('A','mock'), Participant('Ollama-qwen:1b','mock')]
            one = Roundtable('one',seats,directory)
            two = Roundtable('two',seats,directory)
            self.assertNotEqual(one.transcript_path,two.transcript_path)
            one.add_host_message('@Ollama-qwen:1b what do you think?')
            self.assertEqual(one.next_speaker().name,'Ollama-qwen:1b')

    def test_discovery_has_no_sdk_requirement_for_ollama(self):
        with patch('roundtable.config._port_open', side_effect=lambda port: port == 11434), \
             patch('roundtable.config._has_module',return_value=False), \
             patch('roundtable.config._ollama_chat_models',return_value=[('small:1b',1.0),('large:2b',2.0)]):
            seats = discover(include_cli=False)
        self.assertEqual([p.kind for p in seats], ['ollama','ollama'])


class WebTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.seats = [Participant('A','mock'), Participant('B','mock')]
        self.table = Roundtable('test',self.seats,self.directory.name)
        with patch('roundtable.web.blocked', return_value=[]):
            self.hub = Hub(self.table,'test-token')
        self.server = ThreadingHTTPServer(('127.0.0.1',0),_handler_factory(self.hub))
        self.worker = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.worker.start()
        self.url = f'http://127.0.0.1:{self.server.server_port}'

    def tearDown(self):
        self.hub.stopping.set()
        self.hub.auto.clear()
        self.server.shutdown()
        self.server.server_close()
        self.worker.join(timeout=2)
        self.directory.cleanup()

    def request(self, path, body=None, auth=True):
        headers = {'X-Roundtable-Token':'test-token'} if auth else {}
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(self.url+path,data=data,headers=headers)
        try:
            with urllib.request.urlopen(request,timeout=3) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def test_auth_and_malformed_bodies(self):
        self.assertEqual(self.request('/state', auth=False)[0],403)
        self.assertEqual(self.request('/say', [1,2])[0],400)
        self.assertEqual(self.request('/say', {'text':123})[0],400)
        self.assertEqual(self.request('/next', {'speaker':'missing'})[0],400)
        self.assertEqual(self.request('/auto', {'on':'false'})[0],400)
        self.assertEqual(self.request('/say', {'text':'hello'})[0],200)

    def test_bounded_round_and_new_topic(self):
        with patch('roundtable.providers._ADAPTERS', {'mock': lambda *args: iter(['hello'])}):
            self.hub.run_round()
            deadline = time.monotonic()+5
            while self.hub.auto.is_set() and time.monotonic()<deadline:
                time.sleep(.02)
        self.assertFalse(self.hub.auto.is_set())
        self.assertEqual(len(self.table.history),2)
        with patch('roundtable.web.discover',return_value=self.seats), patch('roundtable.web.blocked',return_value=[]):
            status, _ = self.request('/new',{'topic':'A fresh topic'})
        self.assertEqual(status,200)
        self.assertEqual(self.hub.table.topic,'A fresh topic')
        self.assertFalse(self.hub.table.history)
        self.assertTrue(self.table.transcript_path.with_suffix('.md').exists())

    def test_reconnect_snapshot_includes_partial_reply(self):
        self.hub.broadcast({'type':'start','speaker':'A','hex':'#123456'})
        self.hub.broadcast({'type':'chunk','speaker':'A','text':'In progress'})
        self.assertEqual(self.request('/state')[1]['active']['text'],'In progress')

    def test_session_cookie_reconnect_and_cross_origin_write_boundary(self):
        request = urllib.request.Request(self.url + '/session', data=b'{}',
            headers={'X-Roundtable-Token': 'test-token'})
        with urllib.request.urlopen(request) as response:
            cookie = response.headers['Set-Cookie']
        self.assertIn('HttpOnly', cookie)
        self.assertIn('SameSite=Strict', cookie)
        header = cookie.split(';', 1)[0]
        request = urllib.request.Request(self.url + '/state', headers={'Cookie': header})
        with urllib.request.urlopen(request) as response:
            self.assertEqual(json.load(response)['type'], 'snapshot')
        request = urllib.request.Request(self.url + '/say', data=b'{"text":"cookie write"}',
            headers={'Cookie': header})
        with self.assertRaises(urllib.error.HTTPError) as error:
            urllib.request.urlopen(request)
        self.assertEqual(error.exception.code, 403)
        request.add_header('X-Roundtable-Client', 'browser')
        with urllib.request.urlopen(request) as response:
            self.assertTrue(json.load(response)['ok'])


if __name__ == '__main__':
    unittest.main()
