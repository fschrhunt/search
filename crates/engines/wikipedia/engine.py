"""MediaWiki search with article URL construction and readable HTML snippets."""
import json
from urllib.parse import quote, urlencode
from transport import main, row, text


def parse(body, limit):
    """Convert MediaWiki hits, encoding non-ASCII and reserved article characters."""
    results = []
    for hit in json.loads(body)['query']['search']:
        title = hit['title']
        result = row(title, 'https://en.wikipedia.org/wiki/' + quote(title.replace(' ', '_'), safe=''), text(hit.get('snippet', '')))
        if result:
            results.append(result)
    return results[:limit]


def endpoint(query, limit):
    """Query the keyless JSON API with the service's per-query maximum."""
    return 'https://en.wikipedia.org/w/api.php?' + urlencode({'action': 'query', 'list': 'search', 'format': 'json', 'srsearch': query, 'srlimit': min(limit, 20)})


if __name__ == '__main__':
    main(parse, endpoint)
