"""Brave's optional server-rendered scraper; bot challenges are reported as failure."""
from urllib.parse import urlencode
from html_scan import attr, blocks, tag
from transport import main, row


def parse(body, limit):
    """Read web result blocks, titles and snippets; deduplicate by cleaned URL."""
    if 'data-type="web"' not in body and 'no results' not in body.lower():
        raise ValueError('unrecognized Brave markup or challenge')
    results, seen = [], set()
    for block in blocks(body, 'data-type="web"', 'data-type="'):
        hit = row(tag(block, 'div', ['search-snippet-title']) or tag(block, 'a'), attr(block, 'href'), tag(block, 'div', ['line-clamp-dynamic']) or tag(block, 'div', ['generic-snippet']) or None)
        if hit and hit['url'] not in seen:
            seen.add(hit['url'])
            results.append(hit)
    return results[:limit]


def endpoint(query, limit):
    """Encode the query literally for Brave's public HTML endpoint."""
    return 'https://search.brave.com/search?' + urlencode({'q': query})


if __name__ == '__main__':
    main(parse, endpoint, {'user_agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'})
