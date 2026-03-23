from __future__ import annotations

import importlib.metadata
import os
from logging.config import fileConfig

from sqlalchemy import engine_from_config, pool
from alembic import context

from yoink.core.db.base import Base

config = context.config

if config.config_file_name is not None:
    fileConfig(config.config_file_name)

_plugins_env = os.environ.get("YOINK_PLUGINS", "")
_enabled = [n.strip() for n in _plugins_env.split(",") if n.strip()]
for ep in importlib.metadata.entry_points(group="yoink.plugins"):
    if ep.name in _enabled:
        try:
            cls = ep.load()
            inst = cls()
            for model in inst.get_models():
                _ = model.__table__
        except Exception:
            pass

target_metadata = Base.metadata


def _get_url() -> str:
    url = os.environ.get("DATABASE_URL", "postgresql+psycopg://yoink:yoink@localhost:5432/yoink")
    url = url.replace("postgresql+asyncpg://", "postgresql+psycopg://")
    url = url.replace("postgresql://", "postgresql+psycopg://")
    return url


def run_migrations_offline() -> None:
    context.configure(
        url=_get_url(),
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
    )
    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    cfg = config.get_section(config.config_ini_section, {})
    cfg["sqlalchemy.url"] = _get_url()
    connectable = engine_from_config(cfg, prefix="sqlalchemy.", poolclass=pool.NullPool)
    with connectable.connect() as connection:
        context.configure(connection=connection, target_metadata=target_metadata)
        with context.begin_transaction():
            context.run_migrations()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()
