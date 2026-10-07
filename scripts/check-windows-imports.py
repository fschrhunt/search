#!/usr/bin/env python3
"""Reject PE release binaries that require a separate MSVC/UCRT redistributable."""

import re
import struct
import sys
from pathlib import Path


def imports(data):
    """Read ordinary and delay-loaded DLL names from a bounded PE32+ image."""
    def unpack(fmt, offset):
        return struct.unpack_from(fmt, data, offset)

    pe, = unpack('<I', 0x3c)
    if data[:2] != b'MZ' or data[pe:pe + 4] != b'PE\0\0':
        raise ValueError('not a PE executable')
    sections, = unpack('<H', pe + 6)
    optional_size, = unpack('<H', pe + 20)
    optional = pe + 24
    magic, = unpack('<H', optional)
    if magic != 0x20b or optional_size < 240 or sections > 96:
        raise ValueError('expected a bounded PE32+ executable')
    image_base, = unpack('<Q', optional + 24)
    headers_size, = unpack('<I', optional + 60)

    def offset(rva):
        if rva < headers_size and rva < len(data):
            return rva
        for index in range(sections):
            header = optional + optional_size + index * 40
            _, virtual, size, raw = unpack('<IIII', header + 8)
            if virtual <= rva < virtual + size:
                result = raw + rva - virtual
                if result < len(data):
                    return result
        raise ValueError('DLL import points outside the image')

    def name(rva):
        start = offset(rva)
        end = data.find(b'\0', start, min(start + 256, len(data)))
        if end < 0:
            raise ValueError('unterminated DLL name')
        return data[start:end].decode('ascii').lower()

    names = set()
    for directory, stride in [(1, 20), (13, 32)]:
        rva, size = unpack('<II', optional + 112 + directory * 8)
        if rva == 0:
            continue
        for index in range(min(size // stride, 4096)):
            fields = unpack('<' + 'I' * (stride // 4), offset(rva + index * stride))
            if not any(fields):
                break
            if directory == 1:
                names.add(name(fields[3]))
            else:
                names.add(name(fields[1] if fields[0] & 1 else fields[1] - image_base))
        else:
            raise ValueError('unterminated DLL import directory')
    return names


def main():
    """Print native dependencies and fail if a non-system C runtime is needed."""
    names = imports(Path(sys.argv[1]).read_bytes())
    if not names:
        raise ValueError('executable has no DLL import table')
    print('Windows DLL imports:', ', '.join(sorted(names)))
    forbidden = sorted(name for name in names if re.match(
        r'(?:vcruntime|msvcp|msvcr)\d|ucrtbase\.dll$|api-ms-win-crt-', name))
    if forbidden:
        raise SystemExit('external C runtime dependency: ' + ', '.join(forbidden))


if __name__ == '__main__':
    main()
