"""Offline transport and protocol contracts for the copyable custom API engine."""
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2] / 'docs' / 'examples' / 'json-post'
spec = importlib.util.spec_from_file_location('custom', ROOT / 'engine.py')
engine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(engine)


class CustomEngine(unittest.TestCase):
    def test_authenticated_post_preserves_literal_query_and_bounds_results(self):
        """Exercise real request construction, not a vendor-specific schema."""
        response = io.BytesIO(b'{"results":[{"title":"First","url":"https://example.com/"},{"title":"Second"}]}')
        response.headers = {}
        config = {'endpoint': 'https://example.com/search', 'timeout': 7}
        with patch.dict(engine.os.environ, {'SEARCH_API_KEY': 'test-key'}, clear=True), patch.object(engine.urllib.request, 'build_opener') as build:
            build.return_value.open.return_value = response
            rows = engine.search('café & $(literal)', 1, config)
        request = build.return_value.open.call_args.args[0]
        self.assertEqual(request.get_method(), 'POST')
        self.assertEqual(json.loads(request.data), {'query': 'café & $(literal)', 'limit': 1})
        self.assertEqual(request.get_header('Authorization'), 'Bearer test-key')
        self.assertEqual(build.return_value.open.call_args.kwargs, {'timeout': 7})
        self.assertEqual(build.call_args.args[0].proxies, {})
        self.assertEqual(rows, [{'title': 'First', 'url': 'https://example.com/'}])
        with self.assertRaises(ValueError):
            build.call_args.args[1].redirect_request(request, None, 302, '', {}, 'https://example.com/other')

    def test_transport_rejects_unsafe_setup_and_oversized_responses(self):
        """Reject bad setup before networking and cap reads independently of core output."""
        config = {'endpoint': 'https://example.com/search', 'max_response_bytes': 1024}
        with patch.dict(engine.os.environ, {'SEARCH_API_KEY': 'test-key'}, clear=True), patch.object(engine.urllib.request, 'build_opener') as build:
            for changes in ({'endpoint': 'http://example.com/'}, {'endpoint': 'https://user:secret@example.com/'}, {'timeout': True}, {'max_response_bytes': 0}):
                with self.assertRaises(ValueError):
                    engine.search('query', 1, dict(config, **changes))
            build.assert_not_called()
            response = io.BytesIO(b'x' * 1025)
            response.headers = {}
            build.return_value.open.return_value = response
            with self.assertRaises(ValueError):
                engine.search('query', 1, config)

    def test_command_protocol_redacts_failures_and_exchanges_one_answer(self):
        """Run the starter from its own directory with network replaced offline."""
        script = "import engine; engine.search=lambda query,limit,config: [{'title':query,'url':'https://example.com/'}]; engine.main()"
        valid = {'version': 1, 'query': 'literal query', 'limit': 1}
        done = subprocess.run([sys.executable, '-S', '-B', '-c', script], cwd=ROOT, input=json.dumps(valid), text=True, capture_output=True, timeout=5)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(json.loads(done.stdout)['results'][0]['title'], 'literal query')
        for raw in ('secret-invalid-json', json.dumps(dict(valid, limit=True)), json.dumps(dict(valid, version=True))):
            done = subprocess.run([sys.executable, '-S', '-B', 'engine.py'], cwd=ROOT, input=raw, text=True, capture_output=True, timeout=5)
            self.assertNotEqual(done.returncode, 0)
            self.assertEqual(done.stdout, '')
            self.assertEqual(done.stderr, 'engine request failed\n')

        failure = "import engine; engine.search=lambda *args: (_ for _ in ()).throw(ValueError('credential-secret')); engine.main()"
        done = subprocess.run([sys.executable, '-S', '-B', '-c', failure], cwd=ROOT, input=json.dumps(valid), text=True, capture_output=True, timeout=5)
        self.assertNotEqual(done.returncode, 0)
        self.assertEqual(done.stdout, '')
        self.assertEqual(done.stderr, 'engine request failed\n')


if __name__ == '__main__':
    unittest.main()
