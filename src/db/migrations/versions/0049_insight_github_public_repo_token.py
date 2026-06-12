"""insight: github_token_public_repo column

Revision ID: 0049_insight_github_public_repo_token
Revises: 0048_inbox_gh_sync_state
Create Date: 2026-06-12

Adds one nullable column to insight_user_settings:

- github_token_public_repo: stores the OAuth token obtained via a
  separate device-flow exchange against the public_repo-scoped OAuth App.
  NEVER overwrites github_token (the read:user token). The two tokens
  live independently: read:user is used for API reads, public_repo is
  used only for star/unstar mutations.
"""
from alembic import op
import sqlalchemy as sa


revision = "0049_gh_write_token"
down_revision = "0048_inbox_gh_sync_state"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "insight_user_settings",
        sa.Column(
            "github_token_public_repo",
            sa.String(256),
            nullable=True,
            server_default=None,
        ),
    )


def downgrade() -> None:
    op.drop_column("insight_user_settings", "github_token_public_repo")
