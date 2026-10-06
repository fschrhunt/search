#!/usr/bin/env python3
"""Bridge Search's command protocol to a trusted SearXNG endpoint using stdlib.

Set SEARXNG_URL to the /search endpoint. This executable is trusted code and
may contact private destinations; Search's HTTP adapter is the guarded option.
"""

import json
import os
import sys
import urllib.parse
import urllib.request


def main():
    """Read one versioned request, emit one bounded result list, then exit."""
    request = json.load(sys.stdin)
    if request.get("version") != 1:
        raise ValueError("unsupported protocol version")
    endpoint = urllib.parse.urlsplit(os.environ["SEARXNG_URL"])
    if endpoint.scheme not in ("http", "https") or not endpoint.hostname:
        raise ValueError("SEARXNG_URL must be an HTTP(S) endpoint")
    params = [
        (key, value)
        for key, value in urllib.parse.parse_qsl(endpoint.query)
        if key not in ("q", "format")
    ]
    params.extend([("q", request["query"]), ("format", "json")])
    url = urllib.parse.urlunsplit(endpoint._replace(query=urllib.parse.urlencode(params)))
    with urllib.request.urlopen(url, timeout=10) as response:
        body = response.read(1048577)
    if len(body) > 1048576:
        raise ValueError("response too large")
    rows = json.loads(body)["results"]
    results = [
        {"title": row["title"], "url": row["url"], "snippet": row.get("content")}
        for row in rows[: request["limit"]]
    ]
    json.dump({"results": results}, sys.stdout)


if __name__ == "__main__":
    main()
