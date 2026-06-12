"""GitHub Star Lists integration via GraphQL.

Requires a token with `user` scope (the public_repo token from
insight_user_settings.github_token_public_repo is reused; the OAuth App
should be configured with both public_repo and user scopes).

GraphQL surface used (session-verified, 2026-06):
  - user(login:$login) { lists { nodes { id name slug ... } } }
  - createUserList(input:{name,description,isPrivate})
  - updateUserList(input:{listId,name})
  - updateUserListsForItem(input:{itemId,listIds})
  - deleteUserList(input:{listId})

Membership semantics: updateUserListsForItem is a full-replace operation.
Safe add/remove pattern used throughout (read existing ids, then add/remove
target, then write back the merged set).
"""
from __future__ import annotations

import logging
import os
from dataclasses import dataclass, field

import httpx

_PROXY = os.environ.get("proxy_url") or os.environ.get("PROXY_URL") or None

GITHUB_GRAPHQL_URL = "https://api.github.com/graphql"

logger = logging.getLogger(__name__)


@dataclass
class GhList:
    id: str
    name: str
    slug: str
    description: str | None
    is_private: bool
    item_count: int = 0


@dataclass
class GhListsClient:
    token: str
    _client: httpx.AsyncClient = field(init=False, repr=False)

    def __post_init__(self) -> None:
        kwargs: dict = {
            "headers": {
                "Authorization": f"Bearer {self.token}",
                "Content-Type": "application/json",
                "X-Github-Next-Global-ID": "1",
            },
            "timeout": 15,
        }
        if _PROXY:
            kwargs["proxy"] = _PROXY
        self._client = httpx.AsyncClient(**kwargs)

    async def aclose(self) -> None:
        await self._client.aclose()

    async def _gql(self, query: str, variables: dict | None = None) -> dict:
        payload: dict = {"query": query}
        if variables:
            payload["variables"] = variables
        resp = await self._client.post(GITHUB_GRAPHQL_URL, json=payload)
        resp.raise_for_status()
        data = resp.json()
        if errors := data.get("errors"):
            raise RuntimeError(f"GitHub GraphQL error: {errors}")
        return data["data"]

    # ------------------------------------------------------------------
    # Read
    # ------------------------------------------------------------------

    async def get_viewer_login(self) -> str:
        data = await self._gql("query { viewer { login } }")
        return data["viewer"]["login"]

    async def list_user_lists(self, login: str) -> list[GhList]:
        query = """
        query($login: String!) {
          user(login: $login) {
            lists(first: 100) {
              nodes {
                id
                name
                slug
                description
                isPrivate
                items { totalCount }
              }
            }
          }
        }
        """
        data = await self._gql(query, {"login": login})
        nodes = data["user"]["lists"]["nodes"]
        return [
            GhList(
                id=n["id"],
                name=n["name"],
                slug=n["slug"],
                description=n["description"],
                is_private=n["isPrivate"],
                item_count=n["items"]["totalCount"],
            )
            for n in nodes
        ]

    async def get_list_item_ids(self, list_id: str) -> list[str]:
        """Return global node IDs of all Repository items in a list."""
        query = """
        query($listId: ID!) {
          node(id: $listId) {
            ... on UserList {
              items(first: 100) {
                nodes {
                  ... on Repository { id }
                }
                pageInfo { hasNextPage endCursor }
              }
            }
          }
        }
        """
        # TODO: paginate past 100 items if needed
        data = await self._gql(query, {"listId": list_id})
        nodes = data["node"]["items"]["nodes"]
        return [n["id"] for n in nodes if n.get("id")]

    async def get_repo_list_memberships(self, repo_node_id: str) -> list[str]:
        """Return list IDs that this repo currently belongs to."""
        query = """
        query($nodeId: ID!) {
          node(id: $nodeId) {
            ... on Repository {
              starredLists: lists(first: 50) {
                nodes { id }
              }
            }
          }
        }
        """
        data = await self._gql(query, {"nodeId": repo_node_id})
        node = data.get("node") or {}
        lists_data = node.get("starredLists") or {"nodes": []}
        return [n["id"] for n in lists_data["nodes"]]

    # ------------------------------------------------------------------
    # Mutations
    # ------------------------------------------------------------------

    async def create_list(
        self, name: str, description: str = "", is_private: bool = False
    ) -> GhList:
        query = """
        mutation($input: CreateUserListInput!) {
          createUserList(input: $input) {
            list { id name slug description isPrivate }
          }
        }
        """
        data = await self._gql(
            query,
            {"input": {"name": name, "description": description, "isPrivate": is_private}},
        )
        n = data["createUserList"]["list"]
        return GhList(id=n["id"], name=n["name"], slug=n["slug"],
                      description=n["description"], is_private=n["isPrivate"])

    async def update_list(self, list_id: str, name: str) -> None:
        query = """
        mutation($input: UpdateUserListInput!) {
          updateUserList(input: $input) { list { id } }
        }
        """
        await self._gql(query, {"input": {"listId": list_id, "name": name}})

    async def add_repo_to_list(self, repo_node_id: str, list_id: str) -> None:
        """Safe add: read existing memberships, append list_id, write back."""
        existing = await self.get_repo_list_memberships(repo_node_id)
        if list_id in existing:
            return
        desired = existing + [list_id]
        await self._update_item_lists(repo_node_id, desired)

    async def remove_repo_from_list(self, repo_node_id: str, list_id: str) -> None:
        """Safe remove: read existing memberships, subtract list_id, write back."""
        existing = await self.get_repo_list_memberships(repo_node_id)
        desired = [lid for lid in existing if lid != list_id]
        if len(desired) == len(existing):
            return
        await self._update_item_lists(repo_node_id, desired)

    async def _update_item_lists(self, item_id: str, list_ids: list[str]) -> None:
        query = """
        mutation($input: UpdateUserListsForItemInput!) {
          updateUserListsForItem(input: $input) { item { id } }
        }
        """
        await self._gql(query, {"input": {"itemId": item_id, "listIds": list_ids}})
