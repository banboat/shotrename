#!/usr/bin/env python3
"""Build assets/cities.tsv from the GeoNames cities15000 dump.

Usage:
    python3 tools/build_gazetteer.py /path/to/cities15000.txt

The output keeps only the fields ShotRename needs:
    latitude, longitude, name, name_zh, country, population

name_zh is taken from the alternatenames column: a CJK token ending in
"市" (the trailing character is dropped), otherwise the shortest CJK token.
"""

import re
import sys
from pathlib import Path

CJK = re.compile(r"^[㐀-鿿·]{2,}$")
OUT = Path(__file__).resolve().parent.parent / "assets" / "cities.tsv"


def chinese_name(alternates: str) -> str:
    tokens = [token for token in alternates.split(",") if CJK.match(token)]
    for token in tokens:
        if token.endswith("市") and len(token) <= 4:
            return token[:-1]
    for token in tokens:
        if len(token) <= 4:
            return token
    return ""


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    source = Path(sys.argv[1])
    rows = []
    with source.open(encoding="utf-8") as handle:
        for line in handle:
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 15:
                continue
            try:
                lat = float(fields[4])
                lon = float(fields[5])
                population = int(fields[14] or 0)
            except ValueError:
                continue
            name = fields[1].replace("\t", " ")
            zh = chinese_name(fields[3])
            country = fields[8]
            rows.append((population, f"{lat:.4f}\t{lon:.4f}\t{name}\t{zh}\t{country}\t{population}"))

    rows.sort(key=lambda row: (-row[0], row[1]))
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with OUT.open("w", encoding="utf-8") as handle:
        for _, row in rows:
            handle.write(row + "\n")
    print(f"wrote {len(rows)} cities to {OUT} ({OUT.stat().st_size / 1024:.0f} KiB)")


if __name__ == "__main__":
    main()
