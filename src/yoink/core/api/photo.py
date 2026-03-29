"""Shared photo resolution helpers for Bot API local server."""
from __future__ import annotations

import os
from pathlib import Path

import httpx

_TG_FILE_ROOT = "/var/lib/telegram-bot-api/"
_LOCAL_FILE_ROOT = "/app/data/tg-bot-api/"


async def resolve_chat_photo(bot_api_url: str, bot_token: str, chat_id: int) -> bytes | None:
    """
    Fetch the current chat/user photo via getChat → getFile → local file.
    Always calls getChat so the file_id is fresh (stored file_ids expire on photo change).
    Works for both users (private chats) and groups.
    Returns raw JPEG bytes or None if unavailable.
    """
    async with httpx.AsyncClient(timeout=10) as client:
        r = await client.get(
            f"{bot_api_url}/bot{bot_token}/getChat",
            params={"chat_id": chat_id},
        )
        if r.status_code != 200:
            return None
        data = r.json()
        if not data.get("ok"):
            return None
        photo = data["result"].get("photo")
        if not photo or not photo.get("big_file_id"):
            return None
        file_id = photo["big_file_id"]

        r2 = await client.get(
            f"{bot_api_url}/bot{bot_token}/getFile",
            params={"file_id": file_id},
        )
        if r2.status_code != 200:
            return None
        fdata = r2.json()
        if not fdata.get("ok"):
            return None
        file_path: str = fdata["result"].get("file_path", "")

    if file_path.startswith(_TG_FILE_ROOT):
        local = Path(_LOCAL_FILE_ROOT) / file_path[len(_TG_FILE_ROOT):]
        if local.is_file():
            return local.read_bytes()

    async with httpx.AsyncClient(timeout=10) as client:
        r = await client.get(f"{bot_api_url}/file/bot{bot_token}/{file_path}")
        if r.status_code == 200:
            return r.content

    return None


def bot_api_params(app_state: object) -> tuple[str, str]:
    """Return (bot_api_url, bot_token) from app state and environment."""
    bot_api_url = os.environ.get("BOT_API_URL", "https://api.telegram.org")
    bot_token = app_state.settings.bot_token  # type: ignore[attr-defined]
    return bot_api_url, bot_token
