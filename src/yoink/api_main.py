"""ASGI entrypoint for the API service."""
from __future__ import annotations

from yoink.core.api.app import create_api
from yoink.core.config import CoreSettings
from yoink.core.plugin import load_plugins

_config = CoreSettings()
_plugins = load_plugins(_config.yoink_plugins)

app = create_api(_config, _plugins)
