#!/usr/bin/env python3
"""Download attributed snapshots for verify-single; never reads or writes a Rime dictionary."""
from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import gzip
import hashlib
import io
import json
from pathlib import Path
import urllib.request
import zipfile


@dataclass(frozen=True)
class Source:
    id: str
    url: str
    path: str
    revision: str
    license: str
    lineage: str


CNCHAR_REVISION = "b8397db3e08e88ebbf8acf92b5cbab1a7e4c1550"
HANZI_REVISION = "bddc96d41bef78427ed0e034e9f7e31d71fd1b92"
KEYTAO_REVISION = "12333f4d8aa693d72f3ef1a9e25bc3a4fc5d122a"


def sources() -> list[Source]:
    cn = f"https://raw.githubusercontent.com/theajack/cnchar/{CNCHAR_REVISION}/"
    hz = f"https://raw.githubusercontent.com/skishore/makemeahanzi/{HANZI_REVISION}/"
    kt = f"https://raw.githubusercontent.com/xkinput/keytao-next/{KEYTAO_REVISION}/"
    return [
        Source("unihan", "https://www.unicode.org/Public/17.0.0/ucd/Unihan.zip", "unihan-readings.txt", "Unicode 17.0.0", "Unicode-3.0", "Unicode: kMandarin / kHanyuPinyin / kXHC1983 / kHanyuPinlu; fields are not independent votes"),
        Source("unihan_variants", "https://www.unicode.org/Public/17.0.0/ucd/Unihan.zip", "unihan-variants.txt", "Unicode 17.0.0", "Unicode-3.0", "Unicode variant relations for review context only; readings are never automatically inherited"),
        Source("cedict", "https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz", "cedict.txt", "download snapshot; see file header and SHA256", "CC-BY-SA-4.0", "CC-CEDICT community dictionary; single-character headwords only"),
        Source("cnchar_spell", cn + "src/cnchar/main/dict/spell-dict-jian.json", "cnchar-spell.json", CNCHAR_REVISION, "MIT", "README: sourced from Internet, edited/expanded; independence from other datasets unknown; simplified data only"),
        Source("cnchar_order", cn + "src/cnchar/plugin/order/dict/stroke-order-jian.json", "cnchar-order.json", CNCHAR_REVISION, "MIT", "cnchar simplified stroke order; origin not independently established"),
        Source("cnchar_order_trad", cn + "src/cnchar/plugin/trad/dict/stroke-order-trad.json", "cnchar-order-trad.json", CNCHAR_REVISION, "MIT", "cnchar traditional stroke order; no automatic simplified/traditional substitution"),
        Source("cnchar_strokes", cn + "src/cnchar/plugin/order/dict/stroke-table.json", "cnchar-strokes.json", CNCHAR_REVISION, "MIT", "stroke names and shapes used to interpret cnchar order IDs"),
        Source("cnchar_license", cn + "LICENSE", "cnchar-LICENSE", CNCHAR_REVISION, "MIT", "license evidence"),
        Source("hanzi", hz + "dictionary.txt", "hanzi.jsonl", HANZI_REVISION, "LGPL-3.0-or-later", "Derived from Unihan and CJKlib; not an independent Unicode pronunciation vote"),
        Source("hanzi_license", hz + "COPYING", "hanzi-COPYING", HANZI_REVISION, "LGPL-3.0-or-later", "dictionary data LGPL; graphics separately Arphic Public License, not downloaded"),
        Source("hanzi_lgpl", hz + "LGPL", "hanzi-LGPL", HANZI_REVISION, "LGPL-3.0-or-later", "license text"),
        Source("keytao_finals", kt + "config/phonetics-final.csv", "keytao-finals.csv", KEYTAO_REVISION, "scheme mapping reference", "rule facts only; not a pronunciation/shape evidence source"),
        Source("keytao_roots", kt + "config/keytao-root.csv", "keytao-roots.csv", KEYTAO_REVISION, "scheme mapping reference", "rule facts only; no local dictionary codes used as external evidence"),
    ]


def fetch(source: Source) -> tuple[bytes, str]:
    request = urllib.request.Request(source.url, headers={"User-Agent": "xkjd-dict research/0.1"})
    with urllib.request.urlopen(request, timeout=60) as response:
        original = response.read()
    if source.id in ("unihan", "unihan_variants"):
        with zipfile.ZipFile(io.BytesIO(original)) as archive:
            data = archive.read("Unihan_Readings.txt" if source.id == "unihan" else "Unihan_Variants.txt")
    elif source.id == "cedict":
        data = gzip.decompress(original)
    else:
        data = original
    data.decode("utf-8")
    if not data.strip():
        raise ValueError(f"empty source: {source.url}")
    return data, hashlib.sha256(original).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, default=Path(__file__).resolve().parents[1] / ".cache/external")
    parser.add_argument("--refresh", action="store_true", help="redownload, including the rolling CC-CEDICT snapshot")
    args = parser.parse_args()
    args.data_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = args.data_dir / "manifest.json"
    previous = json.loads(manifest_path.read_text()) if manifest_path.exists() else []
    old_by_id = {item["id"]: item for item in previous}
    manifest = []
    for source in sources():
        target = args.data_dir / source.path
        old = old_by_id.get(source.id)
        if (not args.refresh and old and old["url"] == source.url and target.is_file()
                and hashlib.sha256(target.read_bytes()).hexdigest() == old["sha256"]):
            manifest.append(old)
            continue
        try:
            data, archive_hash = fetch(source)
        except Exception as error:
            raise RuntimeError(f"download {source.id} from {source.url}: {error}") from error
        temporary = target.with_suffix(target.suffix + ".tmp")
        temporary.write_bytes(data)
        temporary.replace(target)
        manifest.append({**asdict(source), "sha256": hashlib.sha256(data).hexdigest(), "download_sha256": archive_hash})
        print(f"{source.id}: {len(data)} bytes", flush=True)
    temporary = manifest_path.with_suffix(".tmp")
    temporary.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(manifest_path)


if __name__ == "__main__":
    main()
