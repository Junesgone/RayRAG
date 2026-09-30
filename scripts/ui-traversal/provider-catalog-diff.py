#!/usr/bin/env python3
"""Machine-diff the deployed Available-models panel against RAGFlow's own data.

Upstream `pages/user-setting/setting-model/components/un-add-model.tsx` renders
the cards from `GET /api/v1/providers?available=true`, which
`provider_api_service.list_providers(all_available=True)` derives from
`conf/llm_factories.json`:

  * drop the internal factories (`Youdao`, `FastEmbed`, `BAAI`, `Builtin`,
    `siliconflow_intl`),
  * collect each factory's `model_type` values, add `ocr` to MinerU /
    PaddleOCR / OpenDataLoader,
  * order by `-rank` then name,
  * the client then sorts each card's tags with `orderMap`
    (chat / embedding / rerank / tts / asr / image2text / ocr) and counts the
    `All` + per-tag pills from the search-filtered set.

This script recomputes that expectation from the pinned reference tree and
compares it with the HTML the RayRAG instance serves for `/user-setting/model`
(card order, per-card tags, `ArrowUpRight` doc links and the SystemSetting row
labels). Exit code 1 on any mismatch.

Usage: python3 scripts/ui-traversal/provider-catalog-diff.py [--origin URL]
                                                          [--reference DIR]
"""

import argparse
import http.client
import json
import os
import re
import sys

from urllib.parse import urlsplit
from pathlib import Path

ORDER_MAP = {
    "chat": 1,
    "embedding": 2,
    "rerank": 3,
    "tts": 4,
    "asr": 5,
    "speech2text": 5,
    "image2text": 6,
    "vision": 6,
    "ocr": 7,
}
EXCLUDED = {"Youdao", "FastEmbed", "BAAI", "Builtin", "siliconflow_intl"}
OCR_FACTORIES = {"MinerU", "PaddleOCR", "OpenDataLoader"}

# `constants/llm.ts::APIMapUrl` — the cards render the doc link only for these.
API_MAP_URL = {
    "OpenAI", "Anthropic", "Gemini", "DeepSeek", "Moonshot", "Tongyi-Qianwen",
    "ZHIPU-AI", "xAI", "HuggingFace", "Mistral", "Cohere", "BaiduYiyan",
    "Meituan", "LongCat", "Bedrock", "Azure-OpenAI", "OpenRouter",
    "XunFei Spark", "MiniMax", "Groq", "NVIDIA", "SILICONFLOW", "Replicate",
    "VolcEngine", "Jina", "Tencent Hunyuan", "Tencent Cloud", "ModelScope",
    "Google Cloud", "Fish Audio", "GiteeAI", "StepFun", "BaiChuan", "PPIO",
    "Voyage AI", "TogetherAI", "NovitaAI", "Upstage", "CometAPI", "302.AI",
    "DeerAPI", "TokenPony", "DeepInfra", "PaddleOCR", "SoMark", "n1n",
    "Avian", "Perplexity",
}

SYSTEM_SETTING_LABELS = {
    "en": ["LLM", "Embedding Model", "VLM", "ASR", "Rerank model", "TTS"],
    "zh": ["LLM", "嵌入模型", "VLM", "ASR", "Rerank模型", "TTS"],
}


def expected_providers(factories):
    def model_types(factory):
        # First-seen order, like the upstream API response: duplicates are removed
        # without reordering, so the stable sort below keeps `image2text` before
        # `vision` exactly as the rendered card does.
        types = []
        for llm in factory.get("llm", []):
            raw = llm.get("model_type")
            values = raw if isinstance(raw, list) else ([raw] if raw else [])
            for value in values:
                if value and value not in types:
                    types.append(value)
        return types

    def rank(factory):
        try:
            return -int(factory.get("rank"))
        except (TypeError, ValueError):
            return -500

    rows = []
    for factory in factories:
        if factory["name"] in EXCLUDED:
            continue
        types = model_types(factory)
        if factory["name"] in OCR_FACTORIES and "ocr" not in types:
            types.append("ocr")
        rows.append(
            (
                factory["name"],
                sorted(types, key=lambda value: ORDER_MAP.get(value, 999)),
            )
        )
    ranks = {factory["name"]: rank(factory) for factory in factories}
    rows.sort(key=lambda row: (ranks[row[0]], row[0]))
    return rows


def fetch(url, lang):
    parts = urlsplit(url)
    if parts.scheme not in ("http", "https") or not parts.hostname:
        sys.exit(f"refusing non-http(s) origin: {url}")
    path = parts.path or "/"
    if parts.query:
        path += "?" + parts.query
    connection_class = (
        http.client.HTTPSConnection if parts.scheme == "https" else http.client.HTTPConnection
    )
    connection = connection_class(parts.hostname, parts.port, timeout=60)
    try:
        connection.request(
            "GET", path, headers={"Cookie": f"lang={'zh-CN' if lang == 'zh' else 'en'}"}
        )
        response = connection.getresponse()
        body = response.read().decode("utf-8")
        status = response.status
    except OSError as error:
        sys.exit(f"cannot fetch {url}: {error}")
    finally:
        connection.close()
    if status != 200:
        sys.exit(f"unexpected HTTP {status} from {url}")
    return body


def load_reference(path):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except OSError as error:
        sys.exit(f"cannot read {path}: {error}")
    except json.JSONDecodeError as error:
        sys.exit(f"invalid JSON in {path}: {error}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--origin", default="http://127.0.0.1:9390")
    parser.add_argument(
        "--reference",
        default=os.path.join(
            os.environ.get("HOME", "."), ".cache", "ragflow-v0.27.2"
        ),
    )
    args = parser.parse_args()

    catalog = load_reference(Path(args.reference) / "conf" / "llm_factories.json")
    factories = catalog.get("factory_llm_infos", catalog)
    expected = expected_providers(factories)

    failures = []
    html = fetch(f"{args.origin}/user-setting/model", "en")
    grid = html[html.index("id='availableProviderGrid'") :]
    cards = re.findall(r"data-provider='([^']+)'[^>]*?data-k='([^']*)'", grid)
    got_order = [name for name, _ in cards]
    got = {name: [k for k in kinds.split(" ") if k] for name, kinds in cards}

    if got_order != [name for name, _ in expected]:
        failures.append(
            "card order differs:\n  upstream="
            + ", ".join(name for name, _ in expected[:8])
            + "\n  rayrag  ="
            + ", ".join(got_order[:8])
        )
    for name, types in expected:
        if name not in got:
            failures.append(f"card missing: {name}")
        elif got[name] != types:
            failures.append(
                f"tag order differs for {name}: upstream={' '.join(types)} "
                f"rayrag={' '.join(got[name])}"
            )
    extra = [name for name in got if name not in dict(expected)]
    if extra:
        failures.append("unexpected cards: " + ", ".join(extra))

    # Doc links: `ArrowUpRight` only for the upstream APIMapUrl factories.
    card_blocks = re.findall(
        r"<article class='available-provider-card'.*?</article>", grid, re.S
    )
    for block in card_blocks:
        found = re.search(r"data-provider='([^']+)'", block)
        if not found:
            continue
        name = found.group(1)
        has_link = "available-provider-doc" in block
        if has_link != (name in API_MAP_URL):
            failures.append(
                f"doc link mismatch for {name}: rendered={has_link} "
                f"upstream={name in API_MAP_URL}"
            )

    # SystemSetting row labels (both locales).
    for lang, labels in SYSTEM_SETTING_LABELS.items():
        body = html if lang == "en" else fetch(f"{args.origin}/user-setting/model", "zh")
        section = body[
            body.index("data-testid='system-model-settings'") : body.index(
                "data-testid='added-models-section'"
            )
        ]
        for label in labels:
            if f"{label} <span class='tip-icon'" not in section:
                failures.append(f"{lang}: row label missing: {label}")

    counts = {}
    for _, types in expected:
        for value in set(types):
            counts[value] = counts.get(value, 0) + 1
    print(f"providers: upstream {len(expected)} / rayrag {len(got)}")
    print("per-type counts: " + ", ".join(f"{k} {counts[k]}" for k in sorted(counts)))
    if failures:
        print("\nFAIL")
        for failure in failures:
            print(" - " + failure)
        return 1
    print("\nPASS: order, tags, doc links and row labels all match upstream")
    return 0


if __name__ == "__main__":
    sys.exit(main())
