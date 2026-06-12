"""PTB handlers for the inbox plugin.

Real handlers (URL auto-detect, /save, /inbox, /stars, /stars_sync) land in
subsequent commits. For now this module only exposes an empty list of handler
specs so InboxPlugin.get_handlers() returns cleanly.
"""
from __future__ import annotations


def get_handler_specs() -> list:
    return []
