"""Marginalia's optional old-search HTML scraper; requires its server markup."""
from urllib.parse import urlencode
from html_scan import attr, blocks, tag
from transport import main, row


def parse(body, limit):
    """Read saved card/search-result sections and deduplicate their web URLs."""
    if 'class="card search-result"' not in body and 'no results' not in body.lower():
        raise ValueError('unrecognized Marginalia markup')
    results, seen = [], set()
    for block in blocks(body, 'class="card search-result"', '<section'):
        hit = row(tag(block, 'h2') or tag(block, 'a'), attr(block, 'href'))
        if hit and hit['url'] not in seen:
            seen.add(hit['url'])
            results.append(hit)
    return results[:limit]


def endpoint(query, limit):
    """Use the public old-search endpoint because it returns server-rendered HTML."""
    return 'https://old-search.marginalia.nu/search?' + urlencode({'query': query})


if __name__ == '__main__':
    main(parse, endpoint)
