"""Capture every native DuckDB profile through a pipe, without replaying SQL.

Unlike a single profiling_output file, the pipe cannot overwrite earlier queries.
DuckDB writes when execution completes, including deferred relation fetches. Query
comments carry immutable correlation IDs, so the reader thread never guesses which
span is active. Connections remain native at extension and Splink entry points.
"""

from __future__ import annotations

import codecs
import hashlib
import json
import os
import re
import tempfile
import threading
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any, cast

import duckdb

from er.obs.profiling import context, emit, enabled, span

_TAG = re.compile(r"/\* er_profile:([a-f0-9]+) \*/")
_SENSITIVE = re.compile(r"\b(?:ATTACH|CREATE\s+(?:OR\s+REPLACE\s+)?SECRET)\b", re.I)


class ProfiledConnection:
    def __init__(self, connection: duckdb.DuckDBPyConnection) -> None:
        self.raw = connection
        self.connection_id = uuid.uuid4().hex
        self.current_query_id: str | None = None
        self.current_query_ids: list[str] = []
        self.stream_identity: dict[str, Any] | None = None
        self.storage_logging = False
        if os.environ.get("ER_PROFILE_STORAGE") == "1":
            try:
                self.raw.execute("CALL enable_logging(['DuckLakeMetadata', 'HTTP'])")
                self.storage_logging = True
                emit("storage_logging", connection_id=self.connection_id, status="enabled")
            except duckdb.Error as error:
                emit(
                    "storage_logging",
                    connection_id=self.connection_id,
                    status="unavailable",
                    reason=str(error),
                )
        self.pending: dict[str, dict[str, Any]] = {}
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
        emit("sql_connection", connection_id=self.connection_id)
        self.raw.execute(f"SET profiling_output='{self.pipe_path}'")
        self.raw.execute("SET profiling_mode='detailed'")
        self.raw.execute("SET profiling_coverage='ALL'")
        self.raw.execute("SET enable_profiling='json'")

    def __getattr__(self, name: str) -> Any:
        return getattr(self.raw, name)

    def _tag(self, query: str) -> str:
        statements = self.raw.extract_statements(query)
        pieces = [
            (statement.query, str(statement.type).rsplit(".", 1)[-1]) for statement in statements
        ] or [(query, "EMPTY")]
        self.current_query_ids = []
        tagged = []
        for index, (statement, kind) in enumerate(pieces):
            query_id = uuid.uuid4().hex
            self.current_query_ids.append(query_id)
            self.current_query_id = query_id
            identity = {
                **context(),
                "query_id": query_id,
                "connection_id": self.connection_id,
                "statement_sha256": hashlib.sha256(statement.encode()).hexdigest(),
                "invocation_id": os.environ.get("ER_PROFILE_INVOCATION_ID"),
                "session_id": os.environ.get("ER_PROFILE_SESSION_ID"),
                "pid": os.getpid(),
                "statement_kind": kind,
                "statement_index": index,
                "statement_count": len(pieces),
                "requires_profile": kind in {"INSERT", "DELETE", "UPDATE", "MERGE_INTO", "COPY"}
                or bool(
                    re.search(
                        r"\bCREATE\b[\s\S]*\bTABLE\b[\s\S]*\bAS\s*\(*\s*(SELECT|WITH|FROM)\b",
                        statement,
                        re.I,
                    )
                ),
            }
            self.pending[query_id] = identity
            emit("sql_submitted", **identity)
            tagged.append(f"/* er_profile:{query_id} */ {statement}")
        return ";".join(tagged)

    def _executed(self) -> None:
        for query_id in self.current_query_ids:
            emit("sql_executed", **self.pending[query_id])

    @contextmanager
    def relation_scope(self, identity: dict[str, Any]) -> Iterator[None]:
        # Boundary records share the FIFO with DuckDB's writes. The reader sees
        # the immutable identity BEFORE any profiles from this synchronous native
        # method, even when its generated SQL contains no original comment.
        # No query is replayed and no global/thread context is guessed by _save.
        def marker(value: dict[str, Any] | None) -> None:
            data = (json.dumps({"_er_profile_scope": value}) + "\n").encode()
            while data:
                data = data[os.write(self.write_fd, data) :]

        marker(identity)
        try:
            yield
        finally:
            marker(None)

    def execute(self, query: str, parameters: Any = None, **kwargs: Any) -> ProfiledConnection:
        self.buffered.clear()
        if _SENSITIVE.search(query):
            self.raw.disable_profiling()
            try:
                self.raw.execute(query, parameters, **kwargs)
            finally:
                self.raw.execute("SET enable_profiling='json'")
        else:
            tagged = self._tag(query)
            try:
                self.raw.execute(tagged, parameters, **kwargs)
            except Exception:
                emit("sql_rejected", **self.pending[self.current_query_id or ""])
                raise
            self._executed()
        return self

    def executemany(self, query: str, parameters: Any) -> ProfiledConnection:
        self.buffered.clear()
        with span("sql.parameter_batch", unit="parameter_sets") as metrics:
            metrics["parameter_sets"] = (
                len(parameters) if isinstance(parameters, (list, tuple)) else None
            )
            self.raw.executemany(self._tag(query), parameters)
            self._executed()
        return self

    def sql(self, query: str, **kwargs: Any) -> Any:
        tagged = self._tag(query)
        relation = self.raw.sql(tagged, **kwargs)
        if relation is not None:
            identity = self.pending[self.current_query_id or ""]
            return ProfiledRelation(relation, self, identity)
        self._executed()
        return None

    def query(self, query: str, **kwargs: Any) -> Any:
        return self.sql(query, **kwargs)

    def from_query(self, query: str, **kwargs: Any) -> Any:
        return self.sql(query, **kwargs)

    def fetchone(self) -> Any:
        # Bounded lookahead finalizes scalar queries, while preserving all rows
        # when a caller iterates or switches from fetchone to fetchall.
        if not self.buffered:
            fetched = self.raw.fetchmany(2)
            self.buffered.extend(fetched)
            if len(fetched) < 2:
                self._consumed()
        return self.buffered.pop(0) if self.buffered else None

    def fetchall(self) -> list[Any]:
        rows = self.buffered + self.raw.fetchall()
        self.buffered = []
        self._consumed()
        return rows

    def fetchmany(self, size: int = 1) -> list[Any]:
        rows, self.buffered = self.buffered[:size], self.buffered[size:]
        if len(rows) < size:
            requested = size - len(rows)
            fetched = self.raw.fetchmany(requested)
            rows.extend(fetched)
            if len(fetched) < requested:
                self._consumed()
        return rows

    def _consumed(self) -> None:
        if self.current_query_id:
            emit("sql_consumed", **self.pending[self.current_query_id])

    def cursor(self) -> duckdb.DuckDBPyConnection:
        return instrument_connection(self.raw.cursor())

    def register(self, name: str, value: Any) -> ProfiledConnection:
        # A native relation must stay native at this C API boundary. Otherwise
        # DuckDB sees the proxy's Arrow protocol and can recursively execute the
        # same connection while its registration mutex is held.
        if isinstance(value, ProfiledRelation):
            with self.relation_scope({**value.identity, "relation_operation": "register"}):
                self.raw.register(name, value.raw)
        else:
            self.raw.register(name, value)
        return self

    def _save(self, profile: dict[str, Any]) -> None:
        query = str(profile.get("query_name", ""))
        match = _TAG.search(query)
        identity = self.pending.get(match.group(1)) if match else self.stream_identity
        if identity is None:
            if match:
                raise ValueError("SQL profile has an unknown query identity")
            root = Path(os.environ["ER_PROFILE_DIR"]) / "unattributed"
            root.mkdir(parents=True, exist_ok=True)
            destination = root / f"{self.connection_id}-{uuid.uuid4().hex}.json"
            destination.write_text(
                json.dumps({"connection_id": self.connection_id, "profile": profile})
            )
            exclusion = None
            if re.match(r"^\s*(SET|PRAGMA)\b", query, re.I):
                exclusion = "connection_setting"
            elif re.match(r"^\s*CREATE\s+SCHEMA\b", query, re.I):
                exclusion = "native_schema_setup"
            elif (
                not query.strip()
                and all(
                    node.get("operator_name") == "CREATE_VIEW" and not node.get("children")
                    for node in profile.get("children", [])
                )
                and profile.get("children")
            ):
                exclusion = "native_view_registration"
            emit(
                "sql_unattributed",
                connection_id=self.connection_id,
                statement_kind=query.lstrip().split()[0].upper() if query.strip() else "EMPTY",
                profile_path=str(destination),
                exclusion_reason=exclusion,
            )
            return
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
            bytes_read=profile.get("total_bytes_read"),
            bytes_written=profile.get("total_bytes_written"),
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
                        if "_er_profile_scope" in value:
                            self.stream_identity = value["_er_profile_scope"]
                        else:
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
            if self.storage_logging:
                self._save_storage_logs()
        finally:
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

    def _save_storage_logs(self) -> None:
        self.raw.disable_profiling()
        root = Path(os.environ["ER_PROFILE_DIR"]) / "storage"
        root.mkdir(parents=True, exist_ok=True)
        destination = root / f"{self.connection_id}.jsonl"
        cursor = self.raw.execute(
            "SELECT * FROM duckdb_logs WHERE type IN ('DuckLakeMetadata', 'HTTP')"
        )
        names = [column[0] for column in cursor.description]
        count = 0
        with destination.open("x") as handle:
            for page in iter(lambda: cursor.fetchmany(1024), []):
                for row in page:
                    handle.write(json.dumps(dict(zip(names, row, strict=True)), default=str) + "\n")
                    count += 1
        emit(
            "storage_log",
            connection_id=self.connection_id,
            profile_path=str(destination),
            rows=count,
            status="captured" if count else "unavailable_or_no_events",
        )
        # The generic message is a DuckDB STRUCT rendering, not JSON. Preserve
        # structured timings separately instead of parsing that display string.
        try:
            cursor = self.raw.execute("SELECT * FROM duckdb_logs_parsed('DuckLakeMetadata')")
        except duckdb.Error as error:
            emit(
                "storage_metadata",
                connection_id=self.connection_id,
                status="unavailable",
                reason=str(error),
            )
            return
        names = [column[0] for column in cursor.description]
        destination = root / f"{self.connection_id}-metadata.jsonl"
        count = 0
        with destination.open("x") as handle:
            for page in iter(lambda: cursor.fetchmany(1024), []):
                for row in page:
                    handle.write(json.dumps(dict(zip(names, row, strict=True)), default=str) + "\n")
                    count += 1
        emit(
            "storage_metadata",
            connection_id=self.connection_id,
            profile_path=str(destination),
            rows=count,
            status="captured" if count else "unavailable_or_no_events",
        )


class ProfiledRelation:
    """Preserve lazy relation operations and their originating SQL identity."""

    def __init__(self, relation: Any, owner: ProfiledConnection, identity: dict[str, Any]):
        self.raw = relation
        self.owner = owner
        self.identity = identity

    def _call(self, name: str, *args: Any, **kwargs: Any) -> Any:
        args = tuple(arg.raw if isinstance(arg, ProfiledRelation) else arg for arg in args)
        kwargs = {
            key: value.raw if isinstance(value, ProfiledRelation) else value
            for key, value in kwargs.items()
        }
        identity = {**self.identity, "relation_operation": name}
        with self.owner.relation_scope(identity):
            result = getattr(self.raw, name)(*args, **kwargs)
        if isinstance(result, duckdb.DuckDBPyRelation):
            return ProfiledRelation(result, self.owner, self.identity)
        if name in {"fetchall", "df", "to_df", "fetchdf", "to_arrow_table", "__len__"}:
            emit("sql_consumed", **identity)
        return result

    def __getattr__(self, name: str) -> Any:
        value = getattr(self.raw, name)
        if callable(value):
            return lambda *args, **kwargs: self._call(name, *args, **kwargs)
        return value

    def __len__(self) -> int:
        return int(self._call("__len__"))


def instrument_connection(connection: duckdb.DuckDBPyConnection) -> duckdb.DuckDBPyConnection:
    if not enabled() or os.environ.get("ER_PROFILE_SQL") != "1":
        return connection
    if isinstance(connection, ProfiledConnection):
        return connection
    return cast(duckdb.DuckDBPyConnection, ProfiledConnection(connection))


def raw_connection(connection: duckdb.DuckDBPyConnection) -> duckdb.DuckDBPyConnection:
    return connection.raw if isinstance(connection, ProfiledConnection) else connection
