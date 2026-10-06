"""Defensive scanners preserving the former Brave and Marginalia markup rules."""
import re
from transport import text


def blocks(body, marker, boundary):
    """Yield result fragments after a marker and before their next boundary."""
    for piece in body.split(marker)[1:]:
        end = piece.find(boundary)
        yield piece[:end] if end > 0 else piece


def attr(body, name):
    """Read a quoted attribute, tolerating either HTML quote style."""
    match = re.search(r'\b' + re.escape(name) + r'\s*=\s*([\x22\x27])(.*?)\1', body, re.S)
    return match.group(2) if match else ''


def tag(body, name, classes=()):
    """Return the first matching element's display text, tolerating missing end tags."""
    for match in re.finditer(r'<' + name + r'\b([^>]*)>', body, re.S):
        if all(c in match.group(1) for c in classes):
            tail = body[match.end():]
            end = tail.find('</' + name + '>')
            return text(tail[:end] if end >= 0 else tail)
    return ''
