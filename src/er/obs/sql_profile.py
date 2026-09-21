"""Capture every native DuckDB profile through a pipe, without replaying SQL.

Unlike a single profiling_output file, the pipe cannot overwrite earlier queries.
DuckDB writes when execution completes, including deferred relation fetches. Query
comments carry immutable correlation IDs, so the reader thread never guesses which
span is active. Connections remain native at extension and Splink entry points.
"""

from __future__ import annotations

import codecs
import json
import os
import re
import tempfile
import threading
import uuid
from pathlib import Path
from typing import Any, cast

import duckdb

from er.obs.profiling import context, emit, enabled, span

_TAG = re.compile(r"/\* er_profile:([a-f0-9]+) \*/")
_SENSITIVE = re.compile(r"\b(?:ATTACH|CREATE\s+(?:OR\s+REPLACE\s+)?SECRET)\b", re.I)


class ProfiledConnection:
    def __init__(self, connection: duckdb.DuckDBPyConnection) -> None:
        self.raw = connection
        self.pending: dict[str, dict[str, Any]] = {}
        self.relations: dict[str, dict[str, Any]] = {}
        self.buffered: list[Any] = []
        self.pipe_directory = tempfile.TemporaryDirectory(prefix="er-sql-profile-")
        self.pipe_path = Path(self.pipe_directory.name) / "queries.json"
        os.mkfifo(self.pipe_path)
        self.read_fd = os.open(self.pipe_path, os.O_RDONLY | os.O_NONBLOCK)
        self.write_fd = os.open(self.pipe_path, os.O_WRONLY)
        os.set_blocking(self.read_fd, True)
        self.error: str | None = None
        self.closed = False
        self.thread = threading.Thread(target=self._read, daemon=True, name="er-sql-profiles")
        self.thread.start()
        self.raw.execute("SET enable_profiling='json'")
        self.raw.execute(f"SET profiling_output='{self.pipe_path}'")
        self.raw.execute("SET profiling_mode='detailed'")
        self.raw.execute("SET profiling_coverage='ALL'")
        self.raw.execute("SET enable_profiling='json'")

    def __getattr__(self, name: str) -> Any:
        return getattr(self.raw, name)

    def _tag(self, query: str) -> str:
        query_id = uuid.uuid4().hex
        self.pending[query_id] = {**context(), "query_id": query_id}
        return f"/* er_profile:{query_id} */ {query}"

    def execute(self, query: str, parameters: Any = None, **kwargs: Any) -> ProfiledConnection:
        self.buffered.clear()
        if _SENSITIVE.search(query):
            self.raw.disable_profiling()
            try:
                self.raw.execute(query, parameters, **kwargs)
            finally:
                self.raw.execute("SET enable_profiling='json'")
        else:
            self.raw.execute(self._tag(query), parameters, **kwargs)
        return self

    def executemany(self, query: str, parameters: Any) -> ProfiledConnection:
        self.buffered.clear()
        with span("sql.parameter_batch", unit="parameter_sets") as metrics:
            metrics["parameter_sets"] = (
                len(parameters) if isinstance(parameters, (list, tuple)) else None
            )
            self.raw.executemany(self._tag(query), parameters)
        return self

    def sql(self, query: str, **kwargs: Any) -> Any:
        tagged = self._tag(query)
        relation = self.raw.sql(tagged, **kwargs)
        if relation is not None:
            match = _TAG.search(tagged)
            assert match is not None
            identity = self.pending[match.group(1)]
            self.relations[relation.sql_query()] = identity
        return relation

    def fetchone(self) -> Any:
        # Bounded lookahead finalizes scalar queries, while preserving all rows
        # when a caller iterates or switches from fetchone to fetchall.
        if not self.buffered:
            self.buffered.extend(self.raw.fetchmany(2))
        return self.buffered.pop(0) if self.buffered else None

    def fetchall(self) -> list[Any]:
        rows = self.buffered + self.raw.fetchall()
        self.buffered = []
        return rows

    def fetchmany(self, size: int = 1) -> list[Any]:
        rows, self.buffered = self.buffered[:size], self.buffered[size:]
        if len(rows) < size:
            rows.extend(self.raw.fetchmany(size - len(rows)))
        return rows

    def cursor(self) -> duckdb.DuckDBPyConnection:
        return instrument_connection(self.raw.cursor())

    def _save(self, profile: dict[str, Any]) -> None:
        query = str(profile.get("query_name", ""))
        match = _TAG.search(query)
        identity = self.pending.get(match.group(1)) if match else self.relations.get(query)
        if identity is None:
            if match:
                raise ValueError("SQL profile has an unknown query identity")
            return  # profiler configuration statements have no workload identity
        root = Path(os.environ["ER_PROFILE_DIR"]) / "sql"
        root.mkdir(parents=True, exist_ok=True)
        # executemany can emit several profiles for the same prepared statement.
        destination = root / f"{identity['query_id']}-{uuid.uuid4().hex}.json"
        destination.write_text(json.dumps({**identity, "profile": profile}))
        emit(
            "sql_profile",
            **identity,
            profile_path=str(destination),
            duration_ms=1000 * float(profile.get("latency", 0)),
            rows_returned=profile.get("rows_returned"),
            rows_scanned=profile.get("cumulative_rows_scanned"),
            buffer_peak_bytes=profile.get("system_peak_buffer_memory"),
            spill_peak_bytes=profile.get("system_peak_temp_dir_size"),
            cpu_time_seconds=profile.get("cpu_time"),
        )

    def _read(self) -> None:
        decoder = json.JSONDecoder()
        utf8 = codecs.getincrementaldecoder("utf-8")()
        buffer = ""
        with os.fdopen(self.read_fd, "rb", buffering=0) as stream:
            try:
                while chunk := stream.read(65536):
                    buffer += utf8.decode(chunk)
                    while buffer.strip():
                        buffer = buffer.lstrip()
                        try:
                            value, end = decoder.raw_decode(buffer)
                        except json.JSONDecodeError:
                            break
                        self._save(value)
                        buffer = buffer[end:]
                if buffer.strip():
                    raise ValueError("incomplete DuckDB profile at EOF")
            except Exception as error:
                self.error = f"{type(error).__name__}: {error}"
                # Keep the reader open until close() releases the keeper writer.
                # Otherwise the next native write could block opening the FIFO.
                while stream.read(65536):
                    pass

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        try:
            self.raw.close()
        finally:
            os.close(self.write_fd)
            self.thread.join(timeout=5)
            if self.thread.is_alive():
                self.error = "SQL profile reader did not stop"
            if self.error:
                emit("sql_profile_error", error_detail=self.error)
            self.pipe_directory.cleanup()


def instrument_connection(connection: duckdb.DuckDBPyConnection) -> duckdb.DuckDBPyConnection:
    if not enabled() or os.environ.get("ER_PROFILE_SQL") != "1":
        return connection
    if isinstance(connection, ProfiledConnection):
        return connection
    return cast(duckdb.DuckDBPyConnection, ProfiledConnection(connection))


def raw_connection(connection: duckdb.DuckDBPyConnection) -> duckdb.DuckDBPyConnection:
    return connection.raw if isinstance(connection, ProfiledConnection) else connection
