"""Unit tests for the S5.2 observability layer (S8.1 unit row, S8.4).

Nothing here opens a connection or needs Docker: the counter vocabulary, the log
line's key set and stdout purity are all properties of the CLI skeleton, which is
why they are unit tests. What needs the lake — the `runs` and `run_stages` rows
themselves — lives in ``tests/integration/test_run_metadata.py``.
"""
