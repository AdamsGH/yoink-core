"""In-process metrics counters. Thread-safe, lock-free (asyncio single-thread).

Usage:
    from yoink.core.metrics import metrics
    metrics.inc("downloads_total")
    metrics.inc("cache_hits")
    metrics.observe("download_duration_seconds", 12.3)
    snapshot = metrics.snapshot()
"""
from __future__ import annotations

import time
from dataclasses import dataclass, field


@dataclass
class _Histogram:
    """Simple histogram with count, sum, min, max."""
    count: int = 0
    total: float = 0.0
    min_val: float = float("inf")
    max_val: float = float("-inf")

    def observe(self, value: float) -> None:
        self.count += 1
        self.total += value
        if value < self.min_val:
            self.min_val = value
        if value > self.max_val:
            self.max_val = value

    def to_dict(self) -> dict:
        if self.count == 0:
            return {"count": 0}
        return {
            "count": self.count,
            "sum": round(self.total, 3),
            "avg": round(self.total / self.count, 3),
            "min": round(self.min_val, 3),
            "max": round(self.max_val, 3),
        }


@dataclass
class Metrics:
    """Global metrics store."""
    _counters: dict[str, int] = field(default_factory=dict)
    _histograms: dict[str, _Histogram] = field(default_factory=dict)
    _start_time: float = field(default_factory=time.monotonic)

    def inc(self, name: str, value: int = 1) -> None:
        self._counters[name] = self._counters.get(name, 0) + value

    def get(self, name: str) -> int:
        return self._counters.get(name, 0)

    def observe(self, name: str, value: float) -> None:
        if name not in self._histograms:
            self._histograms[name] = _Histogram()
        self._histograms[name].observe(value)

    def snapshot(self) -> dict:
        uptime = round(time.monotonic() - self._start_time, 1)
        return {
            "uptime_seconds": uptime,
            "counters": dict(self._counters),
            "histograms": {k: v.to_dict() for k, v in self._histograms.items()},
        }

    def reset(self) -> None:
        self._counters.clear()
        self._histograms.clear()
        self._start_time = time.monotonic()


metrics = Metrics()
