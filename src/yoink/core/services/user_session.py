"""
UserSessionService - manages a single Telegram user-mode session via tdlight.

The session token is read from TOKEN_FILE (data/tg-bot-api/user.token).
The file is written by `just tg-login` and never stored in env or config.

Security model:
- Token file: chmod 600, gitignored via data/ rule
- Token never logged (redacted in all log calls)
- Only owner user (config.owner_id) may trigger user-mode commands
- Service is stateless across restarts: reads token fresh on each call
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

import httpx

logger = logging.getLogger(__name__)

_TOKEN_REDACT = 12  # chars to show before redacting


def _redact(token: str) -> str:
    return token[:_TOKEN_REDACT] + "...(redacted)"


class UserSessionError(Exception):
    """Raised when the user session is unavailable or a request fails."""


class UserSessionService:
    """
    Thin async client for tdlight user-mode API calls.

    Usage::

        svc = UserSessionService(base_url="http://host:8082", token_file=Path("data/tg-bot-api/user.token"))
        topics = await svc.get_forum_topics(chat_id=-1001234567890)
    """

    def __init__(
        self,
        base_url: str,
        token_file: Path,
        timeout: float = 30.0,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._token_file = token_file
        self._timeout = timeout

    def _read_token(self) -> str:
        if not self._token_file.exists():
            raise UserSessionError(
                f"User session token not found at {self._token_file}. "
                "Run: just tg-login +<phone>"
            )
        token = self._token_file.read_text().strip()
        if not token:
            raise UserSessionError("User session token file is empty.")
        return token

    def is_available(self) -> bool:
        """Return True if a token file exists and is non-empty."""
        try:
            self._read_token()
            return True
        except UserSessionError:
            return False

    async def call(self, method: str, **params: Any) -> dict:
        """
        Make an authenticated user-mode API call.

        Raises UserSessionError on auth/network failure.
        Raises httpx.HTTPError on unexpected HTTP errors.
        """
        token = self._read_token()
        url = f"{self._base_url}/user{token}/{method}"
        logger.debug("user-session call: %s (token=%s)", method, _redact(token))

        async with httpx.AsyncClient(timeout=self._timeout) as client:
            resp = await client.post(url, data=params)

        try:
            data: dict = resp.json()
        except Exception as exc:
            raise UserSessionError(f"Invalid JSON from tdlight: {resp.text[:200]}") from exc

        if not data.get("ok"):
            code = data.get("error_code", resp.status_code)
            desc = data.get("description", "unknown error")
            if code in (401, 403):
                raise UserSessionError(
                    f"User session unauthorized ({code}): {desc}. "
                    "Re-run: just tg-login +<phone>"
                )
            raise UserSessionError(f"tdlight error {code}: {desc}")

        return data["result"]

    async def get_me(self) -> dict:
        return await self.call("getMe")

    async def get_forum_topics(
        self,
        chat_id: int,
        query: str = "",
        limit: int = 100,
        offset_date: int = 0,
        offset_message_id: int = 0,
        offset_forum_topic_id: int = 0,
    ) -> dict:
        """
        Returns dict with keys: total_count, topics, next_offset_*.
        Each topic has: message_thread_id, name, icon_color, is_closed, is_hidden,
        is_general, is_pinned, unread_count, creation_date.
        """
        return await self.call(
            "getForumTopics",
            chat_id=chat_id,
            query=query,
            limit=limit,
            offset_date=offset_date,
            offset_message_id=offset_message_id,
            offset_forum_topic_id=offset_forum_topic_id,
        )

    async def get_forum_topic(self, chat_id: int, message_thread_id: int) -> dict:
        return await self.call(
            "getForumTopic",
            chat_id=chat_id,
            message_thread_id=message_thread_id,
        )

    async def get_forum_topic_link(self, chat_id: int, message_thread_id: int) -> dict:
        return await self.call(
            "getForumTopicLink",
            chat_id=chat_id,
            message_thread_id=message_thread_id,
        )

    async def search_chat_messages(
        self,
        chat_id: int,
        query: str = "",
        message_thread_id: int = 0,
        from_message_id: int = 0,
        offset: int = 0,
        limit: int = 50,
        filter: str = "",
    ) -> dict:
        params: dict[str, Any] = dict(
            chat_id=chat_id,
            query=query,
            from_message_id=from_message_id,
            offset=offset,
            limit=limit,
        )
        if message_thread_id:
            params["message_thread_id"] = message_thread_id
        if filter:
            params["filter"] = filter
        return await self.call("searchChatMessages", **params)

    async def get_chat_history(
        self,
        chat_id: int,
        from_message_id: int = 0,
        offset: int = 0,
        limit: int = 50,
        only_local: bool = False,
    ) -> dict:
        return await self.call(
            "getChatHistory",
            chat_id=chat_id,
            from_message_id=from_message_id,
            offset=offset,
            limit=limit,
            only_local="true" if only_local else "false",
        )
