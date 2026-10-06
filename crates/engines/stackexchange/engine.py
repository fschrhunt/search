"""Public Stack Overflow search with decoded titles and bounded optional bodies."""
import html
import json
from urllib.parse import urlencode
from transport import main, row, text, truncate


def parse(body, limit):
    """Convert API items; surface API errors/backoff rather than masking them as no hits."""
    value = json.loads(body)
    if value.get('error_id') or value.get('backoff'):
        raise ValueError('Stack Exchange rejected request or requested backoff')
    results = []
    for item in value['items']:
        result = row(html.unescape(item['title']), item['link'], truncate(text(item.get('body_markdown', ''))))
        if result:
            results.append(result)
    return results[:limit]


def endpoint(query, limit):
    """Use the unauthenticated API; quota and backoff remain service constraints."""
    return 'https://api.stackexchange.com/2.3/search/advanced?' + urlencode({'order': 'desc', 'sort': 'relevance', 'q': query, 'site': 'stackoverflow', 'pagesize': min(limit, 30), 'filter': 'default'})


if __name__ == '__main__':
    main(parse, endpoint)
