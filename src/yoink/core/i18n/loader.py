"""i18n YAML loader with plugin locale merging."""
from __future__ import annotations

import logging
from functools import lru_cache
from pathlib import Path
from typing import Any

import yaml

logger = logging.getLogger(__name__)

LOCALES_DIR = Path(__file__).parent / "locales"
DEFAULT_LANG = "en"
SUPPORTED = {"en", "ru"}

_extra_locale_dirs: list[Path] = []


def register_locale_dir(path: Path) -> None:
    """Register an additional locale directory from a plugin."""
    if path not in _extra_locale_dirs:
        _extra_locale_dirs.append(path)
        _load.cache_clear()


@lru_cache(maxsize=32)
def _load(lang: str) -> dict[str, Any]:
    data: dict[str, Any] = {}
    path = LOCALES_DIR / f"{lang}.yml"
    if path.exists():
        with open(path, encoding="utf-8") as f:
            data = yaml.safe_load(f) or {}
    elif lang != DEFAULT_LANG:
        data = dict(_load(DEFAULT_LANG))
    for extra_dir in _extra_locale_dirs:
        extra_path = extra_dir / f"{lang}.yml"
        if extra_path.exists():
            with open(extra_path, encoding="utf-8") as f:
                extra = yaml.safe_load(f) or {}
            _deep_merge(data, extra)
    return data


def _deep_merge(base: dict, override: dict) -> None:
    for k, v in override.items():
        if k in base and isinstance(base[k], dict) and isinstance(v, dict):
            _deep_merge(base[k], v)
        else:
            base[k] = v


def _resolve(data: dict[str, Any], key: str) -> Any:
    node: Any = data
    for part in key.split("."):
        if not isinstance(node, dict):
            return None
        node = node.get(part)
    return node


def t(key: str, lang: str = DEFAULT_LANG, **kwargs: Any) -> str:
    data = _load(lang if lang in SUPPORTED else DEFAULT_LANG)
    value = _resolve(data, key)
    if value is None and lang != DEFAULT_LANG:
        value = _resolve(_load(DEFAULT_LANG), key)
    if value is None:
        logger.debug("Missing i18n key: '%s' (lang=%s)", key, lang)
        return f"[{key}]"
    if not isinstance(value, str):
        return str(value)
    if kwargs:
        try:
            return value.format_map(_SafeDict(kwargs))
        except Exception:
            return value
    return value


class _SafeDict(dict):  # type: ignore[type-arg]
    def __missing__(self, key: str) -> str:
        return f"{{{key}}}"


def clear_cache() -> None:
    _load.cache_clear()
