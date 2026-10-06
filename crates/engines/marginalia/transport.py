"""Shared stdlib command protocol, bounded HTTPS transport, and text helpers.

Each package carries a copy so installed packages need no repository or pip files.
Only explicit command execution calls main; imports and package inspection are inert.
"""
import gzip
import html
import io
import json
import math
import re
import sys
import urllib.parse
import urllib.request


def text(value):
    """Strip markup and decode display entities, preserving literal encoded brackets."""
    return ' '.join(html.unescape(re.sub(r'<[^>]+>', ' ', value)).split())


def truncate(value, size=300):
    """Bound snippets at a nearby word boundary, like the former engine parser."""
    if len(value) <= size:
        return value
    head = value[:size]
    spaces = [i for i, c in enumerate(head) if c.isspace()]
    if spaces and spaces[-1] > size // 2:
        head = head[:spaces[-1]].rstrip()
    return head + '…'


def clean_url(value):
    """Accept public result URL syntax, rejecting credentials and non-web schemes."""
    value = html.unescape(value or '').strip()
    try:
        parsed = urllib.parse.urlsplit(value)
        if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.username or parsed.password:
            return ''
        return value
    except ValueError:
        return ''


def row(title, url, snippet=None):
    """Normalize a result to the strict adapter protocol, skipping unusable rows."""
    url = clean_url(url)
    if not url or not isinstance(title, str) or not title.strip():
        return None
    return {'title': title.strip(), 'url': url, 'snippet': snippet}


class Redirects(urllib.request.HTTPRedirectHandler):
    """Limit redirects to three HTTPS hops on the original service host."""
    def __init__(self, host):
        self.host, self.count = host, 0

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        self.count += 1
        target = urllib.parse.urlsplit(newurl)
        if self.count > 3 or target.scheme != 'https' or target.hostname != self.host or target.username or target.password:
            raise ValueError('redirect rejected')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def fetch(url, config):
    """Fetch a bounded response without environment proxies or unbounded gzip expansion."""
    timeout = config.get('timeout', 15)
    cap = config.get('max_response_bytes', 1048576)
    agent = config.get('user_agent', 'Search/engine-package')
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or not 0 < timeout <= 120:
        raise ValueError('invalid timeout')
    if isinstance(cap, bool) or not isinstance(cap, int) or not 1024 <= cap <= 8388608:
        raise ValueError('invalid response cap')
    if not isinstance(agent, str) or not agent or len(agent) > 1024 or '\r' in agent or '\n' in agent:
        raise ValueError('invalid user agent')
    target = urllib.parse.urlsplit(url)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), Redirects(target.hostname))
    request = urllib.request.Request(url, headers={'User-Agent': agent, 'Accept-Encoding': 'gzip'})
    with opener.open(request, timeout=timeout) as response:
        body = response.read(cap + 1)
        if len(body) > cap:
            raise ValueError('response exceeds cap')
        encoding = response.headers.get('Content-Encoding', '').lower()
        if encoding == 'gzip':
            with gzip.GzipFile(fileobj=io.BytesIO(body)) as compressed:
                body = compressed.read(cap + 1)
        elif encoding not in ('', 'identity'):
            raise ValueError('unsupported encoding')
        if len(body) > cap:
            raise ValueError('response exceeds cap')
        return body.decode('utf-8')


def main(parse, endpoint, defaults=None):
    """Read one bounded version-1 request, emit JSON once, redact every failure."""
    try:
        raw = sys.stdin.buffer.read(1048577)
        if len(raw) > 1048576:
            raise ValueError('request exceeds cap')
        request = json.loads(raw)
        if not isinstance(request, dict) or request.get('version') != 1:
            raise ValueError('unsupported protocol')
        query, limit = request.get('query'), request.get('limit')
        config = request.get('config', {})
        if not isinstance(query, str) or isinstance(limit, bool) or not isinstance(limit, int) or not 1 <= limit <= 1000 or not isinstance(config, dict):
            raise ValueError('invalid request')
        config = dict(defaults or {}, **config)
        results = parse(fetch(endpoint(query, limit), config), limit)
        output = json.dumps({'results': results}, ensure_ascii=False).encode('utf-8')
        if len(output) > 1048576:
            raise ValueError('output exceeds cap')
        sys.stdout.buffer.write(output)
    except Exception:
        sys.stderr.write('engine request failed\n')
        raise SystemExit(1)
