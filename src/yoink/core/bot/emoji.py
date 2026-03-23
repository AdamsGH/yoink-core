"""
Telegram Premium custom emoji helpers.

Custom emoji is only rendered by Telegram when the bot (or user) has
Premium. We always include a plain-text fallback so non-Premium clients
see a normal emoji instead of broken markup.

Usage:
    ce("5368324170671202286", "⭐")
    # renders as <tg-emoji emoji-id="...">⭐</tg-emoji> for Premium users
    # or just ⭐ for others (via fallback=True)
"""
from __future__ import annotations


def ce(emoji_id: str, fallback: str, *, use_custom: bool = False) -> str:
    """Return a custom emoji HTML tag, or the plain fallback if not premium.

    Args:
        emoji_id: The Telegram custom emoji document_id.
        fallback: Plain emoji to display for non-premium or as hint.
        use_custom: Set to True when the sending user has is_premium=True.

    Returns:
        HTML string safe to embed in parse_mode=HTML messages.
    """
    if use_custom:
        return f'<tg-emoji emoji-id="{emoji_id}">{fallback}</tg-emoji>'
    return fallback


# Common emoji document IDs (from Telegram's built-in animated emoji set).
# These are stable across all Telegram clients.
EMOJI_STAR = "5368324170671202286"
EMOJI_CHECK = "5368324170671202286"   # placeholder - update with real IDs
EMOJI_LOCK = "5373141667675261724"
EMOJI_TOOLS = "5373141667675261724"   # placeholder
EMOJI_SHIELD = "5373141667675261724"  # placeholder
