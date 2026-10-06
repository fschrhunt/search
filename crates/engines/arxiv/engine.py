"""arXiv Atom search with namespace-aware parsing and bounded display summaries."""
import xml.etree.ElementTree as ET
from urllib.parse import urlencode
from transport import main, row, truncate


def parse(body, limit):
    """Read Atom entries with or without the namespace used by saved legacy fixtures."""
    if '<!DOCTYPE' in body.upper() or '<!ENTITY' in body.upper():
        raise ValueError('XML entities prohibited')
    root = ET.fromstring(body)
    results = []
    for entry in root.iter():
        if entry.tag.rsplit('}', 1)[-1] != 'entry':
            continue
        fields = {node.tag.rsplit('}', 1)[-1]: ' '.join(''.join(node.itertext()).split()) for node in entry}
        result = row(fields.get('title', ''), fields.get('id'), truncate(fields.get('summary', '')))
        if result:
            results.append(result)
    return results[:limit]


def endpoint(query, limit):
    """Call the HTTPS Atom API with its bounded result count."""
    return 'https://export.arxiv.org/api/query?' + urlencode({'search_query': 'all:' + query, 'max_results': min(limit, 20)})


if __name__ == '__main__':
    main(parse, endpoint)
