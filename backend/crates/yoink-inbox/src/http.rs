//! Router returned here is mounted at /api/v1/inbox. Shared authentication must
//! insert Actor after verifying its bearer/session token. Never accept user IDs
//! from path, query, headers, or bodies as the authenticated actor.
use crate::{
    Error, Result,
    contracts::{Authorization, Insight},
    feed::Filters,
    github::Github,
    pipeline::DEFAULT_PROMPT,
    store::Store,
};
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use yoink_db::query;
#[derive(Clone, Copy)]
pub struct Actor(pub i64);
#[derive(Clone)]
pub struct HttpState {
    pub store: Store,
    pub auth: Arc<dyn Authorization>,
    pub insight: Arc<dyn Insight>,
    pub github: Arc<dyn Github>,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_micros()
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        if let Error::MutationPending(id) = &self {
            return (
                StatusCode::CONFLICT,
                Json(json!({"detail":"GitHub mutation pending reconciliation","mutation_id":id})),
            )
                .into_response();
        }
        let status = match self {
            Error::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
            Error::NotFound => StatusCode::NOT_FOUND,
            Error::Forbidden => StatusCode::FORBIDDEN,
            Error::Uncertain | Error::MutationPending(_) => StatusCode::CONFLICT,
            Error::Cancelled => StatusCode::SERVICE_UNAVAILABLE,
            Error::Completion(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::External => StatusCode::BAD_GATEWAY,
            Error::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Error::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"detail":self.to_string()}))).into_response()
    }
}
pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/gh_mutations", get(mutations))
        .route("/gh_mutations/{id}", get(mutation_status))
        .route("/folders/{id}/rename-remote", post(rename_remote))
        .route("/folders/{id}/delete-remote", post(delete_remote))
        .route("/gh_mutations/{id}/reconcile", post(reconcile_mutation))
        .route(
            "/gh_mutations/{id}/create-candidates",
            get(create_candidates),
        )
        .route("/gh_mutations/{id}/adopt-create", post(adopt_create))
        .route("/gh_mutations/{id}/cancel", post(cancel_mutation))
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/items", get(items).post(create_item))
        .route("/items/{id}", get(item).delete(delete_item))
        .route("/items/{id}/categories", axum::routing::put(assign))
        .route("/items/{id}/reclassify", post(reclassify))
        .route("/categories", get(categories).post(create_category))
        .route(
            "/categories/{id}",
            get(category).put(update_category).delete(delete_category),
        )
        .route("/folders", get(folders).post(create_folder))
        .route(
            "/folders/{id}",
            get(folder)
                .put(update_folder)
                .patch(patch_folder)
                .delete(delete_folder),
        )
        .route("/folders/{id}/stars", post(add_member))
        .route(
            "/folders/{id}/stars/{star}",
            axum::routing::delete(remove_member),
        )
        .route("/rules", get(rules).post(create_rule))
        .route(
            "/rules/{id}",
            get(rule)
                .put(replace_rule)
                .patch(update_rule)
                .delete(delete_rule),
        )
        .route("/teams", get(teams).post(create_team))
        .route(
            "/teams/{id}",
            get(team).put(update_team).delete(delete_team),
        )
        .route("/teams/{id}/members", post(add_team_member))
        .route(
            "/teams/{id}/members/{uid}",
            axum::routing::patch(patch_team_member).delete(remove_team_member),
        )
        .route("/rules/{id}/test", post(test_rule))
        .route("/rules/{id}/run", post(sweep_rule))
        .route("/gh_stars", get(stars))
        .route("/gh_stars/languages", get(languages))
        .route("/settings/classify-hint", get(hint).put(set_hint))
        .route("/admin/classify-prompt", get(prompt).put(set_prompt))
        .route("/gh_stars/sync", post(sync))
        .route(
            "/gh_stars/{id}/star",
            axum::routing::put(star).delete(unstar),
        )
        .with_state(state)
}
async fn require(s: &HttpState, a: Actor, feature: &str) -> Result<()> {
    s.auth.require(a.0, feature).await
}
async fn items(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Query(f): Query<Filters>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.feed(a.0, &f).await?))
}
#[derive(Deserialize)]
struct CreateItem {
    url: String,
    #[serde(default = "web")]
    source: String,
}
fn web() -> String {
    "api".into()
}
async fn create_item(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Json(body): Json<CreateItem>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:ingest").await?;
    let u = url::Url::parse(body.url.trim()).map_err(|_| Error::Invalid)?;
    if !["http", "https"].contains(&u.scheme()) || u.host_str().is_none() {
        return Err(Error::Invalid);
    }
    let r = s.store.ingest(a.0, u.as_str(), &body.source, now()).await?;
    Ok((
        StatusCode::CREATED,
        Json(s.store.item(a.0, r.item_id).await?),
    ))
}
async fn item(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.item(a.0, id).await?))
}
#[derive(Default, Deserialize)]
struct Delete {
    #[serde(default)]
    hard: bool,
}
async fn delete_item(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Query(q): Query<Delete>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:ingest").await?;
    s.store.archive(a.0, id, q.hard, now()).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn assign(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(ids): Json<CategoryIds>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:ingest").await?;
    s.store.assign_categories(a.0, id, &ids.0, now()).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn reclassify(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:ingest").await?;
    require(&s, a, "inbox:classify").await?;
    s.store.enqueue(a.0, Some(id), "classify", now()).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"status":"queued"}))))
}
async fn categories(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.categories(a.0).await?))
}
async fn hint(State(s): State<HttpState>, Extension(a): Extension<Actor>) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(json!({"classify_user_hint":s.store.hint(a.0).await?})))
}
#[derive(Deserialize)]
struct Hint {
    classify_user_hint: Option<String>,
}
async fn set_hint(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Json(b): Json<Hint>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    s.store
        .set_hint(a.0, b.classify_user_hint.as_deref(), now())
        .await?;
    Ok(Json(json!({"classify_user_hint":b.classify_user_hint})))
}
async fn prompt_value(s: &HttpState) -> Result<Value> {
    let value: Option<String> =
        query("SELECT value FROM inbox_admin_settings WHERE key='classify_system_prompt'")
            .fetch_optional(&s.store.db)
            .await?
            .map(|r| r.try_get("value"))
            .transpose()?
            .flatten();
    Ok(json!({"classify_system_prompt":value,"classify_default_prompt":DEFAULT_PROMPT}))
}
async fn prompt(State(s): State<HttpState>, Extension(a): Extension<Actor>) -> Result<Json<Value>> {
    s.auth.require_admin(a.0).await?;
    Ok(Json(prompt_value(&s).await?))
}
#[derive(Deserialize)]
struct Prompt {
    classify_system_prompt: Option<String>,
}
async fn set_prompt(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Json(b): Json<Prompt>,
) -> Result<Json<Value>> {
    s.auth.require_admin(a.0).await?;
    crate::validation::bounded_optional(b.classify_system_prompt.as_deref(), 8000)?;
    query("INSERT INTO inbox_admin_settings(key,value,updated_at) VALUES('classify_system_prompt',?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at").bind(&b.classify_system_prompt).bind(now()).execute(&s.store.db).await?;
    Ok(Json(prompt_value(&s).await?))
}
async fn sync(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:gh_sync").await?;
    s.store.enqueue(a.0, None, "stars_sync", now()).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"status":"queued"}))))
}
fn coordinator(s: &HttpState) -> crate::mutations::Coordinator<'_> {
    crate::mutations::Coordinator {
        store: &s.store,
        auth: &*s.auth,
        insight: &*s.insight,
        github: &*s.github,
    }
}
async fn write_star(s: HttpState, a: Actor, id: i64, add: bool) -> Result<StatusCode> {
    coordinator(&s)
        .execute(
            a.0,
            &crate::mutations::Mutation::Star { star_id: id, add },
            now,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn star(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    write_star(s, a, id, true).await
}
async fn unstar(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    write_star(s, a, id, false).await
}

use crate::{catalog::Catalog, stars::StarFilters};
async fn category(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(
        s.store.catalogue_one(a.0, Catalog::Category, id).await?,
    ))
}
async fn create_category(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Json(b): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:ingest").await?;
    let b = crate::validation::catalogue(&b, Catalog::Category, true)?;
    if b["shared_with_team_id"].as_i64().is_some() {
        require(&s, a, "inbox:share").await?;
    }
    Ok((
        StatusCode::CREATED,
        Json(
            s.store
                .save_catalogue(a.0, Catalog::Category, None, &b, now())
                .await?,
        ),
    ))
}
async fn update_category(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    let b = crate::validation::catalogue(&b, Catalog::Category, true)?;
    if b["shared_with_team_id"].as_i64().is_some() {
        require(&s, a, "inbox:share").await?;
    }
    Ok(Json(
        s.store
            .save_catalogue(a.0, Catalog::Category, Some(id), &b, now())
            .await?,
    ))
}
async fn delete_category(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:ingest").await?;
    s.store.delete_catalogue(a.0, Catalog::Category, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn folders(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.catalogue(a.0, Catalog::Folder).await?))
}
async fn folder(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.catalogue_one(a.0, Catalog::Folder, id).await?))
}

async fn create_folder(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    headers: HeaderMap,
    Json(b): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:ingest").await?;
    let b = crate::validation::catalogue(&b, Catalog::Folder, true)?;
    if b.get("is_local").is_some_and(|v| !v.is_boolean()) {
        return Err(Error::Invalid);
    }
    // Pydantic's executable source defaults is_local to true (the README
    // description alone is not the wire contract).
    let folder = if b["is_local"].as_bool().unwrap_or(true) {
        s.store
            .save_catalogue(a.0, Catalog::Folder, None, &b, now())
            .await?
    } else {
        let key = headers
            .get("Idempotency-Key")
            .map(|v| v.to_str().map_err(|_| Error::Invalid))
            .transpose()?
            .ok_or(Error::Invalid)?;
        let i = coordinator(&s)
            .execute_keyed(
                a.0,
                &crate::mutations::Mutation::CreateList { body: b },
                Some(key),
                now,
            )
            .await?;
        s.store
            .catalogue_one(
                a.0,
                Catalog::Folder,
                i.local_folder_id.ok_or(Error::Uncertain)?,
            )
            .await?
    };
    Ok((StatusCode::CREATED, Json(folder)))
}
async fn update_folder(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    let b = crate::validation::catalogue(&b, Catalog::Folder, true)?;
    Ok(Json(
        s.store
            .save_catalogue(a.0, Catalog::Folder, Some(id), &b, now())
            .await?,
    ))
}
async fn patch_folder(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    let b = crate::validation::folder_patch(&b)?;
    Ok(Json(
        s.store
            .save_catalogue(a.0, Catalog::Folder, Some(id), &b, now())
            .await?,
    ))
}
async fn delete_folder(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:ingest").await?;
    s.store.catalogue_one(a.0, Catalog::Folder, id).await?;
    s.store.delete_catalogue(a.0, Catalog::Folder, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn rename_remote(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> Result<Json<Value>> {
    let b = crate::validation::folder_patch(&b)?;
    if !b.get("name").is_some_and(Value::is_string) {
        return Err(Error::Invalid);
    }
    let i = coordinator(&s)
        .execute(
            a.0,
            &crate::mutations::Mutation::EditList {
                folder_id: id,
                body: b,
            },
            now,
        )
        .await?;
    Ok(Json(
        json!({"id":i.id,"state":i.state,"folder_id":i.local_folder_id}),
    ))
}
async fn delete_remote(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    coordinator(&s)
        .execute(
            a.0,
            &crate::mutations::Mutation::DeleteList { folder_id: id },
            now,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn rules(State(s): State<HttpState>, Extension(a): Extension<Actor>) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.catalogue(a.0, Catalog::Rule).await?))
}
async fn rule(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.catalogue_one(a.0, Catalog::Rule, id).await?))
}
async fn create_rule(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Json(b): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:ingest").await?;
    let b = crate::validation::catalogue(&b, Catalog::Rule, true)?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.store
                .save_catalogue(a.0, Catalog::Rule, None, &b, now())
                .await?,
        ),
    ))
}
async fn update_rule(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(
        s.store
            .save_catalogue(a.0, Catalog::Rule, Some(id), &b, now())
            .await?,
    ))
}
async fn replace_rule(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    let b = crate::validation::catalogue(&b, Catalog::Rule, true)?;
    Ok(Json(
        s.store
            .save_catalogue(a.0, Catalog::Rule, Some(id), &b, now())
            .await?,
    ))
}
async fn delete_rule(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:ingest").await?;
    s.store.delete_catalogue(a.0, Catalog::Rule, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
struct Member {
    star_id: i64,
}

async fn folder_membership(s: &HttpState, a: Actor, id: i64, star: i64, add: bool) -> Result<()> {
    require(s, a, "inbox:ingest").await?;
    let folder = s.store.catalogue_one(a.0, Catalog::Folder, id).await?;
    if folder["gh_list_id"].is_string() {
        coordinator(s)
            .execute(
                a.0,
                &crate::mutations::Mutation::Membership {
                    folder_id: id,
                    star_id: star,
                    add,
                },
                now,
            )
            .await?;
    } else {
        s.store
            .membership_confirmed(a.0, id, star, add, now())
            .await?;
    }
    Ok(())
}
async fn add_member(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Query(b): Query<Member>,
) -> Result<StatusCode> {
    folder_membership(&s, a, id, b.star_id, true).await?;
    Ok(StatusCode::CREATED)
}
async fn remove_member(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path((id, star)): Path<(i64, i64)>,
) -> Result<StatusCode> {
    folder_membership(&s, a, id, star, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn stars(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Query(f): Query<StarFilters>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:gh_sync").await?;
    Ok(Json(s.store.stars(a.0, &f).await?))
}
async fn languages(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:gh_sync").await?;
    Ok(Json(s.store.star_languages(a.0).await?))
}

async fn teams(State(s): State<HttpState>, Extension(a): Extension<Actor>) -> Result<Json<Value>> {
    require(&s, a, "inbox:share").await?;
    Ok(Json(s.store.teams(a.0).await?))
}
async fn team(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:share").await?;
    Ok(Json(s.store.team(a.0, id).await?))
}
async fn create_team(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Json(b): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:share").await?;
    Ok((
        StatusCode::CREATED,
        Json(s.store.save_team(a.0, None, &b, now()).await?),
    ))
}
async fn update_team(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<Value>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:share").await?;
    Ok(Json(s.store.save_team(a.0, Some(id), &b, now()).await?))
}
async fn delete_team(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:share").await?;
    s.store.delete_team(a.0, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
struct TeamMember {
    #[serde(deserialize_with = "crate::validation::deserialize_integer")]
    user_id: i64,
    role: String,
}
async fn add_team_member(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Json(b): Json<TeamMember>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:share").await?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.store
                .team_member(a.0, id, b.user_id, Some(&b.role), now())
                .await?,
        ),
    ))
}
async fn patch_team_member(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path((id, uid)): Path<(i64, i64)>,
    Json(b): Json<TeamMember>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:share").await?;
    if uid != b.user_id {
        return Err(Error::Invalid);
    }
    Ok(Json(
        s.store
            .team_member(a.0, id, uid, Some(&b.role), now())
            .await?,
    ))
}
async fn remove_team_member(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path((id, uid)): Path<(i64, i64)>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:share").await?;
    s.store.team_member(a.0, id, uid, None, now()).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RuleTest {
    item_id: i64,
}
async fn test_rule(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
    Query(q): Query<RuleTest>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:ingest").await?;
    Ok(Json(s.store.test_rule(a.0, id, q.item_id).await?))
}
async fn sweep_rule(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<Value>)> {
    require(&s, a, "inbox:ingest").await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(s.store.sweep_rule(a.0, id, now()).await?),
    ))
}

async fn mutations(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:gh_write").await?;
    Ok(Json(json!(
        s.store
            .active_mutations(a.0)
            .await?
            .iter()
            .map(|i| json!({"id":i.id,"state":i.state,"created_at":i.created_at}))
            .collect::<Vec<_>>()
    )))
}
async fn reconcile_mutation(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let i = coordinator(&s).drive(a.0, &id, now).await?;
    Ok(Json(
        json!({"id":i.id,"state":i.state,"folder_id":i.local_folder_id}),
    ))
}
async fn create_candidates(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(json!(
        coordinator(&s).create_candidates(a.0, &id).await?
    )))
}
#[derive(Deserialize)]
struct AdoptCreate {
    remote_id: String,
}
async fn adopt_create(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<String>,
    Json(b): Json<AdoptCreate>,
) -> Result<Json<Value>> {
    let i = coordinator(&s)
        .adopt_create(a.0, &id, &b.remote_id, now)
        .await?;
    Ok(Json(
        json!({"id":i.id,"state":i.state,"folder_id":i.local_folder_id}),
    ))
}
async fn cancel_mutation(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require(&s, a, "inbox:gh_write").await?;
    s.store.cancel_prepared_mutation(a.0, &id, now()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mutation_status(
    State(s): State<HttpState>,
    Extension(a): Extension<Actor>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    require(&s, a, "inbox:gh_write").await?;
    let i = s.store.mutation(a.0, &id).await?;
    let key = headers
        .get("Idempotency-Key")
        .map(|v| v.to_str().map_err(|_| Error::Invalid))
        .transpose()?;
    if key.is_some_and(|k| k.is_empty() || k.len() > 128) {
        return Err(Error::Invalid);
    }
    // A conflict ID can belong to another request. Prove key ownership without returning the key.
    let operation_key_matches = key.is_some_and(|k| i.request_key.as_deref() == Some(k));
    let create = matches!(i.request, crate::mutations::Mutation::CreateList { .. });
    Ok(Json(
        json!({"id":i.id,"state":i.state,"folder_id":if i.state=="applied"{i.local_folder_id}else{None},"error_code":i.error_code,"created_at":i.created_at,"updated_at":i.updated_at,"can_cancel":i.state=="prepared","can_inspect_candidates":create && ["sending","unknown"].contains(&i.state.as_str()),"operation_key_matches":operation_key_matches}),
    ))
}

#[derive(Deserialize)]
#[serde(transparent)]
struct CategoryIds(#[serde(deserialize_with = "crate::validation::deserialize_integers")] Vec<i64>);
