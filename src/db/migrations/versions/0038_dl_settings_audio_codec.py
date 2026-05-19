"""dl_user_settings: add audio_codec column

Revision ID: 0038_dl_settings_audio_codec
Revises: 0037_user_settings_github_token
Create Date: 2026-05-19
"""
import sqlalchemy as sa
from alembic import op

revision = "0038_dl_settings_audio_codec"
down_revision = "0037_user_settings_github_token"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "dl_user_settings",
        sa.Column("audio_codec", sa.String(16), nullable=False, server_default="best"),
    )


def downgrade() -> None:
    op.drop_column("dl_user_settings", "audio_codec")
