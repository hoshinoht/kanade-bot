"""Generate checked-in portable-contract JSON artifacts."""

import argparse
import json
from pathlib import Path

from .archive import ArchiveManifest
from .composition import Bundle, Manifest, canonical_json
from .config import Catalog, Config
from .crosswalk import (
    RATIONALE,
    RUNTIME_CONFIG_CROSSWALK,
    TABLE_CROSSWALK,
    assert_crosswalk_complete,
)
from .delivery import DeliveryLedger
from .history import History
from .schedule import Schedule

ROOT = Path(__file__).parents[5]
DOCS = ROOT / "legacy" / "python" / "docs" / "bundle-contract"
FIXTURE = ROOT / "legacy" / "python" / "tests" / "fixtures" / "bundle" / "example.json"


def artifacts() -> dict[Path, bytes]:
    assert_crosswalk_complete()
    bundle = Bundle.model_validate_json(FIXTURE.read_bytes())
    sections = {
        "manifest": Manifest,
        "schedule": Schedule,
        "config": Config,
        "history": History,
        "catalog": Catalog,
        "delivery": DeliveryLedger,
    }
    schemas = {
        DOCS / f"{name}-v1.schema.json": json.dumps(
            model.model_json_schema(), indent=2, sort_keys=True
        ).encode()
        + b"\n"
        for name, model in sections.items()
    }
    return schemas | {
        DOCS / "bundle-v1.schema.json": json.dumps(
            Bundle.model_json_schema(), indent=2, sort_keys=True
        ).encode()
        + b"\n",
        DOCS / "archive-v1.schema.json": json.dumps(
            ArchiveManifest.model_json_schema(), indent=2, sort_keys=True
        ).encode()
        + b"\n",
        DOCS / "example.json": canonical_json(bundle) + b"\n",
        DOCS / "crosswalk.json": json.dumps(
            {
                "rationale": RATIONALE,
                "runtime_config": RUNTIME_CONFIG_CROSSWALK,
                "tables": TABLE_CROSSWALK,
            },
            indent=2,
            sort_keys=True,
        ).encode()
        + b"\n",
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    drift = [
        path
        for path, content in artifacts().items()
        if not path.exists() or path.read_bytes() != content
    ]
    if args.check and drift:
        raise SystemExit("generated bundle contract drift: " + ", ".join(map(str, drift)))
    for path, content in artifacts().items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)


if __name__ == "__main__":
    main()
