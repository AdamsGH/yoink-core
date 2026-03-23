"""API key management endpoints (admin/owner only)."""
from __future__ import annotations

from datetime import datetime

from fastapi import APIRouter, Depends, HTTPException, status
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_current_user, get_db
from yoink.core.auth.apikey import ALL_SCOPES, generate_api_key
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import ApiKey, User, UserRole

router = APIRouter(prefix="/api-keys", tags=["api-keys"])


class ApiKeyCreateRequest(BaseModel):
    name: str
    scopes: list[str] = ["*"]
    expires_at: datetime | None = None


class ApiKeyCreateResponse(BaseModel):
    """Returned only once at creation time - contains the raw key."""
    id: int
    name: str
    key: str
    prefix: str
    scopes: list[str]
    expires_at: datetime | None
    created_at: datetime


class ApiKeyResponse(BaseModel):
    id: int
    name: str
    prefix: str
    scopes: list[str]
    created_by: int
    last_used_at: datetime | None
    expires_at: datetime | None
    revoked: bool
    created_at: datetime


class ApiKeyScopesResponse(BaseModel):
    scopes: list[str]


@router.get("/scopes", response_model=ApiKeyScopesResponse)
async def list_available_scopes(
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> ApiKeyScopesResponse:
    return ApiKeyScopesResponse(scopes=ALL_SCOPES)


@router.post("", response_model=ApiKeyCreateResponse, status_code=201)
async def create_api_key(
    body: ApiKeyCreateRequest,
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> ApiKeyCreateResponse:
    for scope in body.scopes:
        if scope != "*" and not scope.endswith(":*") and scope not in ALL_SCOPES:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"Unknown scope: {scope}",
            )

    raw_key, key_hash, prefix = generate_api_key()
    row = ApiKey(
        name=body.name,
        key_hash=key_hash,
        prefix=prefix,
        scopes=body.scopes,
        created_by=current_user.id,
        expires_at=body.expires_at,
    )
    session.add(row)
    await session.commit()
    await session.refresh(row)

    return ApiKeyCreateResponse(
        id=row.id,
        name=row.name,
        key=raw_key,
        prefix=prefix,
        scopes=row.scopes,
        expires_at=row.expires_at,
        created_at=row.created_at,
    )


@router.get("", response_model=list[ApiKeyResponse])
async def list_api_keys(
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[ApiKeyResponse]:
    result = await session.execute(
        select(ApiKey).order_by(ApiKey.created_at.desc())
    )
    return [
        ApiKeyResponse(
            id=k.id,
            name=k.name,
            prefix=k.prefix,
            scopes=k.scopes,
            created_by=k.created_by,
            last_used_at=k.last_used_at,
            expires_at=k.expires_at,
            revoked=k.revoked,
            created_at=k.created_at,
        )
        for k in result.scalars().all()
    ]


@router.delete("/{key_id}", status_code=204)
async def revoke_api_key(
    key_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> None:
    key = await session.get(ApiKey, key_id)
    if key is None:
        raise HTTPException(status_code=404, detail="API key not found")
    key.revoked = True
    await session.commit()
