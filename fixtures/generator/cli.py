"""`python -m fixtures.generator.cli` -- generate one synthetic corpus (S10.1, S10.2).

The entry point the benchmark harness and the fixture-model ticket call. It is
deliberately NOT the `er` CLI of S4.0: `fixtures/` is committed data rather than part
of the `er` distribution (S3), it opens no lake connection and touches no catalog,
and S10.1 puts corpus generation outside every measured phase. Giving it an `er`
subcommand would put a data-authoring tool inside the orchestration contract whose
exit codes S4.0 pins.

`argparse` rather than the `typer` app `src/er/cli.py` builds, for the same reason:
this module is not installed, so it has no console-script entry to hang a Typer app
from, and the whole surface is five options.

Scale arrives as arguments and never as a file. `benchmarks/scales.yaml` (S10.2) does
not exist until M5, and the generator reading a file that names it would make this
module the thing M5 has to change.
"""

from __future__ import annotations

import argparse
from collections.abc import Sequence
from pathlib import Path

from er.config.loader import load_config

from .emit import DEFAULT_CONFIG_PATH, DEFAULT_HOUSEHOLD_RATE, CorpusSpec, emit_corpus
from .personas import generate_personas

__all__ = ["build_parser", "main"]


def build_parser() -> argparse.ArgumentParser:
    """The command line of S10.1's generator."""
    parser = argparse.ArgumentParser(
        prog="python -m fixtures.generator.cli",
        description="Emit a seeded synthetic corpus: one CSV per source, plus truth.csv.",
    )
    parser.add_argument(
        "--personas", type=int, required=True, help="ground-truth persons (S10.2 `Personas`)"
    )
    parser.add_argument(
        "--records",
        type=int,
        required=True,
        help="total rows across the three sources (S10.2 `Records`); must be >= --personas",
    )
    parser.add_argument(
        "--batch",
        type=int,
        default=0,
        help="rows in an additional batch/ delivery (S10.2 `Incremental batch`); 0 emits none",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=None,
        # Defaulted from the config rather than from a literal: the seed is a config
        # field (S6 `generator.seed`), and a literal here would be a second statement
        # of it that could disagree with the document the run is hashed against.
        help="RNG seed; `generator.seed` from --config when omitted",
    )
    parser.add_argument(
        "--out", type=Path, required=True, help="directory to write the corpus into"
    )
    parser.add_argument(
        "--config",
        type=Path,
        default=DEFAULT_CONFIG_PATH,
        help="the S6 document supplying generator.seed and sources.<name>.columns",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Generate the corpus and return the process exit code.

    Returns:
        ``0``. Every failure path -- a bad scale, an unreadable config, a profile
        naming an unknown source -- raises, so the caller sees a traceback naming the
        offending value rather than a bare non-zero code. This is a developer tool
        run from a Makefile and a workflow step, not the S4.0 CLI whose exit codes a
        pipeline branches on.
    """
    args = build_parser().parse_args(argv)
    config = load_config(args.config)
    seed = args.seed if args.seed is not None else config.generator.seed
    spec = CorpusSpec(
        seed=seed,
        personas=args.personas,
        records=args.records,
        batch=args.batch,
        household_rate=DEFAULT_HOUSEHOLD_RATE,
    )
    personas = generate_personas(spec.seed, spec.personas, spec.household_rate)
    for path in emit_corpus(spec, personas, args.out, config=config):
        print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
