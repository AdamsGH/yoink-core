"""Bounded source prose only. No classification, rule loading or policy in Python."""
from failures import MediaFailure


def bounded_metadata(info):
    if not isinstance(info, dict):
        raise MediaFailure("invalid_media")
    def text(key, limit, cap=False):
        value = info.get(key)
        if value is None:
            value = ""
        if not isinstance(value, str):
            raise MediaFailure("invalid_media")
        if cap:
            return value[:limit]
        if len(value) > limit:
            raise MediaFailure("resource_limit")
        return value
    def terms(key):
        value = info.get(key)
        if value is None:
            value = []
        if not isinstance(value, (list, tuple)):
            raise MediaFailure("invalid_media")
        if len(value) > 100 or any(not isinstance(v, str) or len(v) > 128 for v in value):
            raise MediaFailure("resource_limit")
        return list(value)
    return {"title": text("title", 2000), "description": text("description", 2000, True),
            "tags": terms("tags"), "categories": terms("categories")}


class MetadataCollector:
    """Conservative whole-batch prose (also covers playlist clips and gallery zip).

    We never drop later items to make the envelope fit: oversized aggregate prose
    stops the worker. Descriptions alone have the legacy 2000-character cap.
    """
    def __init__(self):
        self._seen = set()
        self.metadata = None

    def add(self, info):
        value = bounded_metadata(info)
        key = (value["title"], value["description"], tuple(value["tags"]), tuple(value["categories"]))
        if key in self._seen:
            return
        if len(self._seen) >= 100:
            raise MediaFailure("resource_limit")
        self._seen.add(key)
        if self.metadata is None:
            self.metadata = value
            return
        merged = {name: " ".join(v for v in [self.metadata[name], value[name]] if v)
                  for name in ("title", "description")}
        merged.update({name: list(dict.fromkeys(self.metadata[name] + value[name]))
                       for name in ("tags", "categories")})
        if len(merged["description"]) > 2000:
            raise MediaFailure("resource_limit")
        self.metadata = bounded_metadata(merged)

    def finish(self):
        if self.metadata is None:
            raise MediaFailure("invalid_media")
        return self.metadata
