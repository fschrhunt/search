"""Optional DuckDuckGo HTML scraper; no official API or stable markup guarantee."""
from html.parser import HTMLParser
from urllib.parse import parse_qs, urlencode, urlsplit
from transport import main, row, text


class Results(HTMLParser):
    """Collect result anchors/snippets while tolerating nested emphasis tags."""
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.hits, self.current, self.capture = [], None, None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        classes = attrs.get('class', '').split()
        if 'result__a' in classes:
            self.current = {'title': '', 'url': attrs.get('href', ''), 'snippet': ''}
            self.hits.append(self.current)
            self.capture = 'title'
        elif 'result__snippet' in classes and self.current is not None:
            self.capture = 'snippet'

    def handle_endtag(self, tag):
        if tag == 'a' or (tag == 'div' and self.capture == 'snippet'):
            self.capture = None

    def handle_data(self, data):
        if self.current is not None and self.capture:
            self.current[self.capture] += data


def parse(body, limit):
    """Unwrap uddg redirect links; reject challenges/unrecognized layouts explicitly."""
    if 'result__a' not in body and 'no results' not in body.lower():
        raise ValueError('DuckDuckGo challenge or unrecognized markup')
    parser = Results()
    parser.feed(body)
    results, seen = [], set()
    for item in parser.hits:
        url = item['url']
        parsed = urlsplit(url)
        if parsed.hostname == 'duckduckgo.com' and parsed.path.startswith('/l/'):
            url = parse_qs(parsed.query).get('uddg', [''])[0]
        result = row(text(item['title']), url, text(item['snippet']) or None)
        if result and result['url'] not in seen:
            seen.add(result['url'])
            results.append(result)
    return results[:limit]


def endpoint(query, limit):
    """Use the optional HTML endpoint; do not bypass bot challenges."""
    return 'https://html.duckduckgo.com/html/?' + urlencode({'q': query})


if __name__ == '__main__':
    main(parse, endpoint)
