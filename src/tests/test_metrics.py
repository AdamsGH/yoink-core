"""Unit tests for the metrics module - no DB needed."""
from __future__ import annotations

from yoink.core.metrics import Metrics


class TestMetrics:

    def test_counter_inc(self):
        m = Metrics()
        m.inc("foo")
        m.inc("foo")
        m.inc("foo", 3)
        assert m.get("foo") == 5

    def test_counter_default_zero(self):
        m = Metrics()
        assert m.get("nonexistent") == 0

    def test_histogram_observe(self):
        m = Metrics()
        m.observe("latency", 1.0)
        m.observe("latency", 3.0)
        m.observe("latency", 2.0)
        snap = m.snapshot()
        h = snap["histograms"]["latency"]
        assert h["count"] == 3
        assert h["min"] == 1.0
        assert h["max"] == 3.0
        assert h["avg"] == 2.0

    def test_snapshot_has_uptime(self):
        m = Metrics()
        snap = m.snapshot()
        assert "uptime_seconds" in snap
        assert snap["uptime_seconds"] >= 0

    def test_reset(self):
        m = Metrics()
        m.inc("a", 10)
        m.observe("b", 5.0)
        m.reset()
        assert m.get("a") == 0
        snap = m.snapshot()
        assert snap["counters"] == {}
        assert snap["histograms"] == {}
