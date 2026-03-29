"""cookie unique constraint includes is_pool

Revision ID: 0021_cookie_unique_constraint
Revises: 0020_cookie_pool_flag
Create Date: 2026-03-29
"""
from alembic import op

revision = "0021_cookie_unique_constraint"
down_revision = "0020_cookie_pool_flag"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.drop_constraint("cookies_user_id_domain_key", "cookies", type_="unique")
    op.create_unique_constraint(
        "cookies_user_id_domain_is_pool_key",
        "cookies",
        ["user_id", "domain", "is_pool"],
    )


def downgrade() -> None:
    op.drop_constraint("cookies_user_id_domain_is_pool_key", "cookies", type_="unique")
    op.create_unique_constraint(
        "cookies_user_id_domain_key",
        "cookies",
        ["user_id", "domain"],
    )
