"""Small shared helpers for inbox PTB handlers."""
from __future__ import annotations

import re
from typing import TYPE_CHECKING

from telegram import MessageEntity

if TYPE_CHECKING:
    from telegram import Message

_URL_RE = re.compile(r"https?://[^\s\)\]\>\"\']+", re.IGNORECASE)


def extract_first_url(message: "Message | None") -> str | None:
    """Return the first URL found in `message`, looking at entities then text.

    Mirrors yoink-dl's `extract_url`, kept local so inbox does not take a hard
    dep on the dl plugin (insight is a real dep; dl is not).
    """
    if message is None:
        return None
    if message.entities:
        for entity in message.entities:
            if entity.type == MessageEntity.TEXT_LINK and entity.url:
                return entity.url
            if entity.type == MessageEntity.URL and message.text:
                start = entity.offset
                end = entity.offset + entity.length
                return message.text[start:end]
    text = message.text or message.caption or ""
    if (m := _URL_RE.search(text)) is not None:
        return m.group(0)
    return None


def extract_url_from_args_or_reply(text_args: list[str], message: "Message") -> str | None:
    """For `/save <url>` style commands: prefer the explicit arg, fall back to the replied message."""
    for arg in text_args:
        if (m := _URL_RE.search(arg)) is not None:
            return m.group(0)
    if message.reply_to_message is not None:
        return extract_first_url(message.reply_to_message)
    return None
