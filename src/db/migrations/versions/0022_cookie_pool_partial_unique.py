"""cookie personal unique constraint as partial index (is_pool=false only)

Pool cookies allow multiple rows per (user_id, domain) - one per account.
Personal cookies remain unique per (user_id, domain).

Revision ID: 0022_cookie_pool_partial_unique
Revises: 0021_cookie_unique_constraint
Create Date: 2026-03-29
"""
from alembic import op
import sqlalchemy as sa

revision = "0022_cookie_pool_partial_unique"
down_revision = "0021_cookie_unique_constraint"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.drop_constraint("cookies_user_id_domain_is_pool_key", "cookies", type_="unique")
    op.execute(
        "CREATE UNIQUE INDEX cookies_personal_unique "
        "ON cookies (user_id, domain) WHERE is_pool = false"
    )
    op.add_column("cookies", sa.Column("label", sa.String(128), nullable=True))


def downgrade() -> None:
    op.drop_column("cookies", "label")
    op.execute("DROP INDEX cookies_personal_unique")
    op.create_unique_constraint(
        "cookies_user_id_domain_is_pool_key",
        "cookies",
        ["user_id", "domain", "is_pool"],
    )
