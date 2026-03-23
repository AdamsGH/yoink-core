"""Telegram initData HMAC verification (Bot API spec)."""
from __future__ import annotations

import hashlib
import hmac
import time
from urllib.parse import parse_qsl, unquote


_MAX_AGE_SECONDS = 86400  # 24 hours


def verify_init_data(init_data: str, bot_token: str, max_age: int = _MAX_AGE_SECONDS) -> dict:
    """
    Verify Telegram WebApp initData HMAC signature.

    Returns parsed fields dict on success.
    Raises ValueError with a descriptive message on any verification failure.

    Spec: https://core.telegram.org/bots/webapps#validating-data-received-via-the-mini-app
    """
    params = dict(parse_qsl(init_data, keep_blank_values=True))

    received_hash = params.pop("hash", None)
    if not received_hash:
        raise ValueError("initData missing hash field")

    # Build data-check string: sorted key=value pairs joined by \n, hash excluded
    data_check_string = "\n".join(
        f"{k}={unquote(v)}" for k, v in sorted(params.items())
    )

    secret_key = hmac.new(b"WebAppData", bot_token.encode(), hashlib.sha256).digest()
    expected_hash = hmac.new(secret_key, data_check_string.encode(), hashlib.sha256).hexdigest()

    if not hmac.compare_digest(expected_hash, received_hash):
        raise ValueError("initData HMAC signature mismatch")

    auth_date_str = params.get("auth_date")
    if auth_date_str:
        try:
            age = int(time.time()) - int(auth_date_str)
            if age > max_age:
                raise ValueError(f"initData expired: {age}s old (max {max_age}s)")
        except (TypeError, ValueError) as exc:
            if "expired" in str(exc):
                raise
            raise ValueError(f"initData auth_date invalid: {auth_date_str}") from exc

    return params
