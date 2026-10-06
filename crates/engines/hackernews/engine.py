"""Algolia Hacker News search, retaining discussion-only story URL fallbacks."""
import json
from urllib.parse import quote, urlencode
from transport import main, row, text


def parse(body, limit):
    """Prefer external story URLs; otherwise use objectID for an HN discussion."""
    results = []
    for hit in json.loads(body)['hits']:
        url = hit.get('url')
        if not url and hit.get('objectID') is not None:
            url = 'https://news.ycombinator.com/item?id=' + quote(str(hit['objectID']), safe='')
        result = row(hit.get('title') or 'HN discussion', url, text(hit.get('story_text') or '') or None)
        if result:
            results.append(result)
    return results[:limit]


def endpoint(query, limit):
    """Encode one Algolia query without interpolating executable text."""
    return 'https://hn.algolia.com/api/v1/search?' + urlencode({'query': query, 'hitsPerPage': min(limit, 30)})


if __name__ == '__main__':
    main(parse, endpoint)
