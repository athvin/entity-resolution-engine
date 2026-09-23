"""Offline, pinned BGE ONNX benchmark provider; downloads only via explicit --bake."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from importlib.metadata import version
from pathlib import Path
from typing import Any

REPOSITORY = "BAAI/bge-small-en-v1.5"
REVISION = "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a"
FILES = {
    "onnx/model.onnx": "828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35",
    "tokenizer.json": "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
    "config.json": "094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750",
    "tokenizer_config.json": "9261e7d79b44c8195c1cada2b453e55b00aeb81e907a6664974b4d7776172ab3",
}
MAX_TOKENS = 64
BATCH_SIZE = 64


def doctor(directory: Path) -> dict[str, Any]:
    from packaging.requirements import Requirement

    pins = {}
    for line in Path(__file__).with_name("vector-requirements.lock").read_text().splitlines():
        if not line or line.startswith((" ", "#")):
            continue
        requirement = Requirement(line.removesuffix("\\").strip())
        if requirement.marker is not None and not requirement.marker.evaluate():
            continue
        actual = version(requirement.name)
        if actual not in requirement.specifier:
            raise RuntimeError(f"{requirement.name}: {actual} violates {requirement.specifier}")
        pins[requirement.name] = actual
    for name, expected in FILES.items():
        with (directory / name).open("rb") as handle:
            actual = hashlib.file_digest(handle, "sha256").hexdigest()
        if actual != expected:
            raise RuntimeError(f"model checksum mismatch: {name}")
    return {
        "repository": REPOSITORY,
        "revision": REVISION,
        "files": FILES,
        "pins": pins,
        "pooling": "cls",
        "normalization": "l2",
        "dimensions": 384,
        "max_tokens": MAX_TOKENS,
        "batch_size": BATCH_SIZE,
        "quantization": "none",
    }


def bake(directory: Path) -> None:
    """Build-time operation, never called by inference."""
    import urllib.request

    for name, expected in FILES.items():
        destination = directory / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(
            f"https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{name}", timeout=180
        ) as source:
            data = source.read()
        if hashlib.sha256(data).hexdigest() != expected:
            raise RuntimeError(f"download checksum mismatch: {name}")
        destination.write_bytes(data)
    (directory / "manifest.json").write_text(
        json.dumps(doctor(directory), sort_keys=True, indent=2)
    )


def onnx_keys(
    connection: Any,
    inputs: str,
    directory: Path,
    *,
    seed: int,
    bands: int,
    bits: int,
) -> str:
    """Deduplicate in SQL; cross the Python inference boundary in bounded batches."""
    import numpy as np
    import onnxruntime as ort
    from tokenizers import Tokenizer

    doctor(directory)
    if not 1 <= bits <= 63 or bands < 1:
        raise ValueError("bands must be positive and bits must be in 1..63")
    os.environ["TOKENIZERS_PARALLELISM"] = "false"
    options = ort.SessionOptions()
    options.intra_op_num_threads = int(os.environ.get("ER_DUCKDB_THREADS", "2"))
    options.inter_op_num_threads = 1
    session = ort.InferenceSession(
        str(directory / "onnx/model.onnx"), sess_options=options, providers=["CPUExecutionProvider"]
    )
    tokenizer = Tokenizer.from_file(str(directory / "tokenizer.json"))
    tokenizer.enable_truncation(max_length=MAX_TOKENS)
    tokenizer.enable_padding(pad_id=0, pad_token="[PAD]")
    projections = (
        np.random.Generator(np.random.PCG64(seed))
        .standard_normal((384, bands * bits))
        .astype(np.float32)
    )
    connection.execute(
        "CREATE OR REPLACE TEMP TABLE bench_onnx_keys "
        "(input_id VARCHAR, band INTEGER, value VARCHAR)"
    )
    from er.lake.bulk import staged_query

    with staged_query(
        connection,
        "SELECT *, row_number() OVER (ORDER BY length(text), input_id) AS input_ordinal "
        f"FROM {inputs}",
    ) as ordered:
        offset = 0
        while batch := connection.execute(
            f"SELECT input_id,text FROM {ordered} "
            "WHERE input_ordinal>? AND input_ordinal<=? ORDER BY input_ordinal",
            [offset, offset + BATCH_SIZE],
        ).fetchall():
            encoded = tokenizer.encode_batch([row[1] for row in batch])
            arrays = {
                "input_ids": np.array([e.ids for e in encoded], dtype=np.int64),
                "attention_mask": np.array([e.attention_mask for e in encoded], dtype=np.int64),
                "token_type_ids": np.array([e.type_ids for e in encoded], dtype=np.int64),
            }
            output = session.run(
                None, {item.name: arrays[item.name] for item in session.get_inputs()}
            )[0]
            vectors = output[:, 0, :] if output.ndim == 3 else output
            if vectors.shape != (len(batch), 384) or not np.isfinite(vectors).all():
                raise RuntimeError(f"invalid model output: {vectors.shape}")
            norms = np.linalg.norm(vectors, axis=1, keepdims=True)
            if (norms == 0).any():
                raise RuntimeError("zero model vector")
            signs = (vectors / norms @ projections >= 0).reshape(len(batch), bands, bits)
            keys = (
                signs.astype(np.uint64) * (np.uint64(1) << np.arange(bits, dtype=np.uint64))
            ).sum(axis=2)
            # Benchmark-only inference exception. No source records are looped over
            # in production; these are distinct view texts and their stored keys.
            connection.execute(
                "INSERT INTO bench_onnx_keys SELECT unnest(?::VARCHAR[]), "
                "unnest(?::INTEGER[]), unnest(?::VARCHAR[])",
                [
                    [item[0] for item in batch for _ in range(bands)],
                    list(range(bands)) * len(batch),
                    [str(int(key)) for key in keys.ravel()],
                ],
            )
            offset += len(batch)
    return "bench_onnx_keys"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bake", type=Path, help="build-time download and checksum verification")
    parser.add_argument("--doctor", type=Path, help="offline runtime/artifact verification")
    arguments = parser.parse_args()
    if arguments.bake is not None:
        bake(arguments.bake)
    if arguments.doctor is not None:
        print(json.dumps(doctor(arguments.doctor), indent=2))
    if arguments.bake is None and arguments.doctor is None:
        parser.error("choose --bake or --doctor")
