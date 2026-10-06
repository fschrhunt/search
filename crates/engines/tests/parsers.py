"""Offline saved-response and command-protocol contract for maintained packages."""
import contextlib
import gzip
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / 'tests' / 'fixtures'


def load(name):
    """Import an installed-style package without bytecode writes or execution."""
    sys.dont_write_bytecode = True
    for module in ('transport', 'html_scan'):
        sys.modules.pop(module, None)
    sys.path.insert(0, str(ROOT / name))
    try:
        spec = importlib.util.spec_from_file_location('engine_' + name, ROOT / name / 'engine.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    finally:
        sys.path.pop(0)


class Parsers(unittest.TestCase):
    def test_saved_brave_and_marginalia_markup_preserves_titles_and_snippets(self):
        """Use the actual legacy saved markup, including markup cleanup and URL filtering."""
        brave = load('brave').parse((FIXTURES / 'brave.html').read_text(), 10)
        self.assertTrue(brave)
        self.assertEqual(brave[0]['title'], 'Ingress | Kubernetes')
        self.assertTrue(brave[0]['snippet'])
        marginalia = load('marginalia').parse((FIXTURES / 'marginalia.html').read_text(), 10)
        self.assertTrue(marginalia)
        self.assertTrue(all(r['url'].startswith('http') for r in marginalia))

    def test_wikipedia_constructs_encoded_urls_and_strips_snippet_markup(self):
        """Protect the transformation that a plain HTTP pointer mapping cannot express."""
        hits = load('wikipedia').parse((FIXTURES / 'wikipedia.json').read_text(), 10)
        self.assertEqual(hits[0], {'title': 'Kubernetes', 'url': 'https://en.wikipedia.org/wiki/Kubernetes', 'snippet': 'an orchestrator'})
        self.assertEqual(hits[1]['url'], 'https://en.wikipedia.org/wiki/Caf%C3%A9_%26_Rust')
        self.assertEqual(hits[1]['snippet'], 'An & entity')

    def test_hackernews_keeps_discussion_fallbacks_and_skips_orphans(self):
        """Retain objectID fallbacks and the legacy title for untitled discussions."""
        hits = load('hackernews').parse((FIXTURES / 'hackernews.json').read_text(), 10)
        self.assertEqual(len(hits), 2)
        self.assertEqual(hits[0]['snippet'], 'Some & text')
        self.assertEqual(hits[1], {'title': 'HN discussion', 'url': 'https://news.ycombinator.com/item?id=11', 'snippet': None})

    def test_stackexchange_decodes_titles_and_honors_backoff(self):
        """Keep entity decoding and reject API backoff/error objects explicitly."""
        module = load('stackexchange')
        hits = module.parse((FIXTURES / 'stackexchange.json').read_text(), 1)
        self.assertEqual(hits[0]['title'], 'Rust & lifetimes')
        self.assertEqual(hits[0]['snippet'], 'Some body')
        for value in ({'error_id': 502}, {'backoff': 5, 'items': []}):
            with self.assertRaises(ValueError):
                module.parse(json.dumps(value), 10)

    def test_arxiv_namespaces_whitespace_and_bounded_summaries(self):
        """Cover Atom namespaces, the original bare feed, and entity expansion refusal."""
        module = load('arxiv')
        body = (FIXTURES / 'arxiv.xml').read_text()
        hits = module.parse(body, 10)
        self.assertEqual(hits[0], {'title': 'A Title', 'url': 'http://arxiv.org/abs/1', 'snippet': 'Sum'})
        self.assertEqual(module.parse(body.replace(' xmlns="http://www.w3.org/2005/Atom"', ''), 10), hits)
        long = '<feed><entry><id>https://arxiv.org/abs/1</id><title>T</title><summary>' + 'word ' * 100 + '</summary></entry></feed>'
        self.assertLessEqual(len(module.parse(long, 1)[0]['snippet']), 301)
        with self.assertRaises(ValueError):
            module.parse('<!DOCTYPE x [<!ENTITY y "boom">]><feed/>', 1)

    def test_duckduckgo_unwraps_redirects_and_reports_challenges(self):
        """Pin known markup without implying live service availability."""
        module = load('duckduckgo')
        hits = module.parse((FIXTURES / 'duckduckgo.html').read_text(), 1)
        self.assertEqual(hits[0], {'title': 'A result & title', 'url': 'https://example.com/article', 'snippet': 'A useful & snippet'})
        for name in ('brave', 'marginalia', 'duckduckgo'):
            with self.assertRaises(ValueError):
                load(name).parse('<html>captcha</html>', 1)

    def test_commands_exchange_one_literal_request_with_no_network(self):
        """Patch only fetch in subprocesses: exercise installed cwd, stdin EOF and stdout JSON."""
        fixtures = {'brave':'brave.html', 'marginalia':'marginalia.html', 'wikipedia':'wikipedia.json', 'hackernews':'hackernews.json', 'stackexchange':'stackexchange.json', 'arxiv':'arxiv.xml', 'duckduckgo':'duckduckgo.html'}
        script = "import engine,transport,sys; body=open(sys.argv[1]).read(); transport.fetch=lambda url,config: body; transport.main(engine.parse,engine.endpoint)"
        for name, fixture in fixtures.items():
            with self.subTest(name=name):
                request = {'version':1, 'query':'$(echo secret) & café', 'limit':1, 'config':{}}
                done = subprocess.run([sys.executable, '-S', '-B', '-c', script, str(FIXTURES / fixture)], cwd=ROOT/name, input=json.dumps(request), capture_output=True, text=True, timeout=5)
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertEqual(len(json.loads(done.stdout)['results']), 1)

    def test_protocol_and_transport_failures_never_expose_input_or_output(self):
        """Invalid input and parser exceptions must exit nonzero with redacted stderr."""
        script = "import engine,transport; transport.fetch=lambda *a: 'secret-response'; transport.main(engine.parse,engine.endpoint)"
        for request in ('secret-invalid-json', json.dumps({'version':1,'query':'credential-secret','limit':1}), json.dumps({'version':2,'query':'secret','limit':1}), json.dumps({'version':1,'query':'secret','limit':True})):
            done = subprocess.run([sys.executable, '-S', '-B', '-c', script], cwd=ROOT/'wikipedia', input=request, capture_output=True, text=True, timeout=5)
            self.assertNotEqual(done.returncode, 0)
            self.assertEqual(done.stdout, '')
            self.assertEqual(done.stderr, 'engine request failed\n')

    def test_transport_bounds_compressed_bodies_and_rejects_external_redirects(self):
        """Test the command's own transport guards without live HTTP."""
        load('wikipedia')
        transport = sys.modules['transport']
        class Response(io.BytesIO):
            headers = {'Content-Encoding':'gzip'}
        class Opener:
            def open(self, *args, **kwargs):
                return Response(gzip.compress(b'x'*2000))
        original = transport.urllib.request.build_opener
        transport.urllib.request.build_opener = lambda *args: Opener()
        try:
            with self.assertRaises(ValueError):
                transport.fetch('https://example.com/', {'max_response_bytes':1024})
            for timeout in (float('nan'), 0, True):
                with self.assertRaises(ValueError):
                    transport.fetch('https://example.com/', {'timeout':timeout})
        finally:
            transport.urllib.request.build_opener = original
        redirects = transport.Redirects('example.com')
        request = transport.urllib.request.Request('https://example.com/')
        for target in ('http://example.com/', 'https://other.invalid/', 'https://user:secret@example.com/'):
            with self.assertRaises(ValueError):
                redirects.redirect_request(request, None, 302, '', {}, target)

    def test_packaged_helpers_match_the_maintained_source(self):
        """Prevent accidental divergence among the self-contained package helper copies."""
        for manifest in ROOT.glob('*/engine.json'):
            m = json.loads(manifest.read_text())
            for helper in ('transport.py', 'html_scan.py'):
                if helper in m['files']:
                    self.assertEqual((manifest.parent/helper).read_bytes(), (ROOT/'support'/helper).read_bytes())


if __name__ == '__main__':
    unittest.main()
