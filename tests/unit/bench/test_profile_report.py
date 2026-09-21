import importlib
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "benchmarks"))
resource_window = importlib.import_module("profile_report").resource_window


def test_phase_memory_uses_current_samples_in_its_own_window() -> None:
    samples = [
        {
            "monotonic_ns": 1,
            "memory.current": 100,
            "memory.peak": 900,
            "process_rss_bytes": {"10": 80},
        },
        {
            "monotonic_ns": 2,
            "memory.current": 500,
            "memory.peak": 900,
            "process_rss_bytes": {"10": 100, "20": 300},
        },
        {
            "monotonic_ns": 3,
            "memory.current": 200,
            "memory.peak": 900,
            "process_rss_bytes": {"10": 80},
        },
    ]
    first = resource_window(samples, 1, 1)
    second = resource_window(samples, 2, 3)
    assert first["sampled_memory_peak_bytes"] == 100
    assert second["sampled_memory_peak_bytes"] == 500
    assert second["sampled_process_rss_sum_peak_bytes"] == 400
    assert resource_window(samples, 4, 5)["sampled_memory_peak_bytes"] is None
