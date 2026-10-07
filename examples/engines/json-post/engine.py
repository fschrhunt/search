"""Custom JSON POST starter, not a vendor integration. Adapt payload and normalize.

Read one version-1 stdin request; emit bounded results or a redacted failure.
Executable engines are trusted host code, not covered by Search's SSRF guard.
"""
import json
import math
import os
import sys
import urllib.parse
import urllib.request


class NoRedirects(urllib.request.HTTPRedirectHandler):
    """Keep the credential on the configured endpoint."""
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('redirect refused')


def payload(query, limit):
    """Map Search's query and result count to your API's request fields."""
    return {'query': query, 'limit': limit}


def normalize(answer, limit):
    """Map your API's response to titles, absolute URLs and optional snippets.

    For AI APIs, join highlights/excerpts here; Search fetches full pages separately.
    Search validates rows and assigns engine attribution and ranking.
    """
    if not isinstance(answer, dict) or 'error' in answer or not isinstance(answer.get('results'), list):
        raise ValueError('invalid response')
    return answer['results'][:limit]


def search(query, limit, config):
    """POST authenticated JSON with bounded I/O, no proxies and no redirects."""
    endpoint = config['endpoint']
    url = urllib.parse.urlsplit(endpoint)
    if url.scheme != 'https' or not url.hostname or url.username is not None or url.password is not None:
        raise ValueError('endpoint must be HTTPS without embedded credentials')
    url.port
    key = os.environ['SEARCH_API_KEY']
    if not key or '\r' in key or '\n' in key:
        raise ValueError('invalid credential')
    timeout = config.get('timeout', 30)
    cap = config.get('max_response_bytes', 1048576)
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or not 0 < timeout <= 120:
        raise ValueError('invalid timeout')
    if isinstance(cap, bool) or not isinstance(cap, int) or not 1024 <= cap <= 8388608:
        raise ValueError('invalid response cap')
    data = json.dumps(payload(query, limit)).encode('utf-8')
    if len(data) > 1048576:
        raise ValueError('request too large')
    request = urllib.request.Request(endpoint, data=data, headers={
        'Content-Type': 'application/json', 'Accept': 'application/json',
        'User-Agent': 'search-custom-engine', 'Authorization': 'Bearer ' + key,
    })
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirects())
    with opener.open(request, timeout=timeout) as response:
        if response.headers.get('Content-Encoding', '').lower() not in ('', 'identity'):
            raise ValueError('unsupported encoding')
        raw = response.read(cap + 1)
    if len(raw) > cap:
        raise ValueError('response too large')
    return normalize(json.loads(raw), limit)


def main():
    """Exchange one bounded command-protocol request, never logging secrets."""
    try:
        raw = sys.stdin.buffer.read(1048577)
        if len(raw) > 1048576:
            raise ValueError('input too large')
        request = json.loads(raw)
        if not isinstance(request, dict) or type(request.get('version')) is not int or request['version'] != 1:
            raise ValueError('invalid protocol')
        query, limit, config = request['query'], request['limit'], request.get('config', {})
        if not isinstance(query, str) or not query.strip() or type(limit) is not int or not 1 <= limit <= 100 or not isinstance(config, dict):
            raise ValueError('invalid request')
        answer = json.dumps({'results': search(query, limit, config)}, ensure_ascii=False).encode('utf-8')
        if len(answer) > 1048576:
            raise ValueError('output too large')
        sys.stdout.buffer.write(answer)
    except Exception:
        sys.stderr.write('engine request failed\n')
        raise SystemExit(1)


if __name__ == '__main__':
    main()
