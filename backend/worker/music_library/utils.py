"""Shared utilities."""
from __future__ import annotations

import difflib
import re
from urllib.parse import parse_qs, urlencode, urlparse, urlunparse

_CYR_RE = re.compile(r"[а-яёА-ЯЁ]")
_TRAILING_URL_PUNCTUATION = "\"'«»()[]<>.,!?:;"


def strip_url_punctuation(url: str) -> str:
    """Remove punctuation commonly attached to a URL in prose."""
    return url.rstrip(_TRAILING_URL_PUNCTUATION)


def _to_latin(s: str) -> str:
    """Transliterate Cyrillic characters to Latin; leave Latin unchanged."""
    try:
        from transliterate import translit
        return translit(s, "ru", reversed=True)
    except Exception:
        return s


def _norm(s: str) -> str:
    import unicodedata
    if _CYR_RE.search(s):
        s = _to_latin(s)
    # Decompose unicode (Ø -> O + combining stroke) then drop combining marks
    s = unicodedata.normalize("NFKD", s)
    s = "".join(c for c in s if not unicodedata.combining(c))
    s = s.lower()
    # Strip all parenthetical suffixes: (feat. X), (Official Video), (Live 2025),
    # (Remaster), (ft. X), etc. These add noise when comparing against clean
    # metadata from Spotify/Deezer. Brackets too.
    s = re.sub(r"\([^)]*\)", "", s)
    s = re.sub(r"\[[^\]]*\]", "", s)
    s = re.sub(r"\s+", " ", s)
    return s.strip()


def score(a: str, b: str) -> float:
    """Similarity score [0..1] between two normalized strings."""
    return difflib.SequenceMatcher(None, _norm(a), _norm(b)).ratio()


_MIN_TITLE = 0.5
_MIN_ARTIST = 0.4


def track_score(
    candidate_artist: str,
    candidate_title: str,
    expected_artist: str,
    expected_title: str,
) -> float:
    """Combined match score for a track candidate.

    Returns the geometric mean of title and artist similarity.
    Both components must clear their individual floors (_MIN_TITLE,
    _MIN_ARTIST) or the function returns 0.0 - this prevents a
    perfect artist match from carrying a completely wrong title
    over the adapter threshold.
    """
    import math
    ts = score(candidate_title, expected_title)
    if not expected_artist and not candidate_artist:
        return ts
    as_ = score(candidate_artist, expected_artist)
    if ts < _MIN_TITLE or (expected_artist and as_ < _MIN_ARTIST):
        return 0.0
    return math.sqrt(ts * as_)


def normalize_url(url: str) -> str:
    """Strip tracking params (si, utm_*, nd, context) for consistent cache keys."""
    url = strip_url_punctuation(url)
    parsed = urlparse(url)
    if not parsed.query:
        return url
    filtered = {
        k: v
        for k, v in parse_qs(parsed.query, keep_blank_values=True).items()
        if not k.startswith("utm_") and k not in ("si", "context", "nd")
    }
    return urlunparse(parsed._replace(query=urlencode(filtered, doseq=True)))
