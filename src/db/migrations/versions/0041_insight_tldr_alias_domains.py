"""insight_tldr_aliases: domain bindings + binding-to-built-in rows

Revision ID: 0041_tldr_alias_domains
Revises: 0040_insight_tldr_aliases_multi
Create Date: 2026-05-26

Adds:
  - domains      VARCHAR(512) NULL  -- csv glob list, e.g. "xda-developers.com, *.lwn.net"
  - target_alias VARCHAR(32)  NULL  -- when set, row binds domains to a built-in
                                       alias (max/nobullshit/...); prompt may be NULL.

Loosens:
  - aliases  -> NULL allowed (pure domain-binding rows do not need a keyword)
  - prompt   -> NULL allowed (pure domain-binding rows reuse the built-in prompt)

A CHECK constraint guarantees every row carries at least one of:
  (a) a user prompt + alias keyword, or
  (b) a target_alias (built-in binding), or
  (c) a domains list (pure URL match against a built-in or alias keyword).
"""
import sqlalchemy as sa
from alembic import op

revision = "0041_tldr_alias_domains"
down_revision = "0040_insight_tldr_aliases_multi"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "insight_tldr_aliases",
        sa.Column("domains", sa.String(512), nullable=True),
    )
    op.add_column(
        "insight_tldr_aliases",
        sa.Column("target_alias", sa.String(32), nullable=True),
    )
    op.alter_column("insight_tldr_aliases", "aliases", existing_type=sa.String(256), nullable=True)
    op.alter_column("insight_tldr_aliases", "prompt", existing_type=sa.Text(), nullable=True)
    op.create_check_constraint(
        "ck_insight_tldr_alias_shape",
        "insight_tldr_aliases",
        "(aliases IS NOT NULL AND prompt IS NOT NULL) "
        "OR (target_alias IS NOT NULL) "
        "OR (domains IS NOT NULL)",
    )


def downgrade() -> None:
    op.drop_constraint("ck_insight_tldr_alias_shape", "insight_tldr_aliases", type_="check")
    # Restore NOT NULL only if data permits; pre-existing rows from this migration
    # may carry NULLs. For a clean downgrade, drop the new rows first.
    op.execute(
        "DELETE FROM insight_tldr_aliases "
        "WHERE aliases IS NULL OR prompt IS NULL"
    )
    op.alter_column("insight_tldr_aliases", "prompt", existing_type=sa.Text(), nullable=False)
    op.alter_column("insight_tldr_aliases", "aliases", existing_type=sa.String(256), nullable=False)
    op.drop_column("insight_tldr_aliases", "target_alias")
    op.drop_column("insight_tldr_aliases", "domains")
