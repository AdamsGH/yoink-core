"""Error hierarchy."""
from __future__ import annotations


class YoinkError(Exception):
    """Base exception for all yoink errors."""


class ConfigError(YoinkError):
    """Misconfigured settings."""


class PluginError(YoinkError):
    """Plugin loading or execution error."""
