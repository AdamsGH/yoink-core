"""Thread policy management + forum topic scan via user-mode session."""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Request, status
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_db
from yoink.core.api.exceptions import ConflictError, NotFoundError
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import Group, ThreadPolicy, User, UserRole
from yoink.core.services.user_session import UserSessionError, UserSessionService

router = APIRouter(prefix="/threads", tags=["threads"])


class ThreadPolicyResponse(BaseModel):
    id: int
    group_id: int
    thread_id: int | None
    name: str | None
    enabled: bool


class ThreadPolicyCreate(BaseModel):
    group_id: int
    thread_id: int | None = None
    name: str | None = None
    enabled: bool = True


class ThreadPolicyUpdate(BaseModel):
    name: str | None = None
    enabled: bool | None = None


class ScanResult(BaseModel):
    group_id: int
    total_count: int
    upserted: int
    topics: list[dict]


def _svc(request: Request) -> UserSessionService:
    svc: UserSessionService | None = None
    if hasattr(request.app.state, "bot_data"):
        svc = request.app.state.bot_data.get("user_session")
    if svc is None:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="User-mode session service not available",
        )
    return svc


@router.get("/group/{group_id}", response_model=list[ThreadPolicyResponse])
async def list_thread_policies(
    group_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> list[ThreadPolicyResponse]:
    group = await session.get(Group, group_id)
    if group is None:
        raise NotFoundError("Group not found")
    rows = (
        await session.execute(
            select(ThreadPolicy)
            .where(ThreadPolicy.group_id == group_id)
            .order_by(ThreadPolicy.thread_id)
        )
    ).scalars().all()
    return [
        ThreadPolicyResponse(
            id=p.id, group_id=p.group_id, thread_id=p.thread_id,
            name=p.name, enabled=p.enabled,
        )
        for p in rows
    ]


@router.post("", response_model=ThreadPolicyResponse, status_code=status.HTTP_201_CREATED)
async def create_thread_policy(
    body: ThreadPolicyCreate,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> ThreadPolicyResponse:
    group = await session.get(Group, body.group_id)
    if group is None:
        raise NotFoundError("Group not found")
    existing = (
        await session.execute(
            select(ThreadPolicy).where(
                ThreadPolicy.group_id == body.group_id,
                ThreadPolicy.thread_id == body.thread_id,
            )
        )
    ).scalar_one_or_none()
    if existing is not None:
        raise ConflictError("Thread policy already exists for this group/thread")
    policy = ThreadPolicy(
        group_id=body.group_id,
        thread_id=body.thread_id,
        name=body.name,
        enabled=body.enabled,
    )
    session.add(policy)
    await session.commit()
    await session.refresh(policy)
    return ThreadPolicyResponse(
        id=policy.id, group_id=policy.group_id, thread_id=policy.thread_id,
        name=policy.name, enabled=policy.enabled,
    )


@router.patch("/{policy_id}", response_model=ThreadPolicyResponse)
async def update_thread_policy(
    policy_id: int,
    body: ThreadPolicyUpdate,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> ThreadPolicyResponse:
    policy = await session.get(ThreadPolicy, policy_id)
    if policy is None:
        raise NotFoundError("Thread policy not found")
    if body.name is not None:
        policy.name = body.name
    if body.enabled is not None:
        policy.enabled = body.enabled
    await session.commit()
    await session.refresh(policy)
    return ThreadPolicyResponse(
        id=policy.id, group_id=policy.group_id, thread_id=policy.thread_id,
        name=policy.name, enabled=policy.enabled,
    )


@router.delete("/{policy_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_thread_policy(
    policy_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> None:
    policy = await session.get(ThreadPolicy, policy_id)
    if policy is None:
        raise NotFoundError("Thread policy not found")
    await session.delete(policy)
    await session.commit()


@router.post("/scan/{group_id}", response_model=ScanResult)
async def scan_forum_topics(
    group_id: int,
    request: Request,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> ScanResult:
    """
    Fetch all forum topics for a group via user-mode session and upsert
    their names into ThreadPolicy. Requires `just tg-login` to be done first.
    """
    svc = _svc(request)
    if not svc.is_available():
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="User-mode session not configured. Run: just tg-login +<phone>",
        )

    group = await session.get(Group, group_id)
    if group is None:
        raise NotFoundError("Group not found")

    try:
        all_topics: list[dict] = []
        offset_date = 0
        offset_message_id = 0
        offset_forum_topic_id = 0

        while True:
            result = await svc.get_forum_topics(
                chat_id=group_id,
                limit=100,
                offset_date=offset_date,
                offset_message_id=offset_message_id,
                offset_forum_topic_id=offset_forum_topic_id,
            )
            batch = result.get("topics", [])
            all_topics.extend(batch)

            if (
                len(batch) < 100
                or not result.get("next_offset_date")
            ):
                break
            offset_date = result["next_offset_date"]
            offset_message_id = result.get("next_offset_message_id", 0)
            offset_forum_topic_id = result.get("next_offset_forum_topic_id", 0)

    except UserSessionError as exc:
        raise HTTPException(status_code=status.HTTP_502_BAD_GATEWAY, detail=str(exc)) from exc

    upserted = 0
    for topic in all_topics:
        thread_id: int = topic["message_thread_id"]
        name: str = topic.get("name", "")
        existing = (
            await session.execute(
                select(ThreadPolicy).where(
                    ThreadPolicy.group_id == group_id,
                    ThreadPolicy.thread_id == thread_id,
                )
            )
        ).scalar_one_or_none()
        if existing is None:
            session.add(ThreadPolicy(
                group_id=group_id,
                thread_id=thread_id,
                name=name,
                enabled=True,
            ))
        else:
            existing.name = name
        upserted += 1

    await session.commit()

    return ScanResult(
        group_id=group_id,
        total_count=len(all_topics),
        upserted=upserted,
        topics=all_topics,
    )
