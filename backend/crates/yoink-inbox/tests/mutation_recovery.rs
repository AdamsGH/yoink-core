mod support;
use serde_json::json;
use support::*;
use yoink_db::{Database, query};
use yoink_inbox::{
    Error,
    catalog::Catalog,
    github::Github,
    mutations::{Coordinator, Mutation},
    store::Store,
};
fn service<'a>(s: &'a Store, f: &'a Fixture) -> Coordinator<'a> {
    Coordinator {
        store: s,
        auth: &Auth,
        insight: &Keys,
        github: &f.client,
    }
}
#[tokio::test]
async fn ambiguous_create_is_not_replayed_and_explicit_adoption_survives_restart() {
    let (d, s) = db().await;
    let f = Fixture::new().await;
    f.list("existing", "Projects").await;
    f.state.lock().await.drop_after = Some("create".into());
    let request = Mutation::CreateList {
        body: json!({"name":"Projects","is_pinned":true,"sort_order":9}),
    };
    let err = service(&s, &f)
        .execute(1, &request, || 10)
        .await
        .err()
        .unwrap();
    let Error::MutationPending(id) = err else {
        panic!("must preserve unknown")
    };
    assert_eq!(f.state.lock().await.writes, vec!["create"]);
    assert_eq!(s.catalogue(1, Catalog::Folder).await.unwrap(), json!([]));
    assert!(s.mutation(2, &id).await.is_err());
    s.db.close().await.unwrap();
    let s = Store::new(
        Database::open(
            &d.path().join("db"),
            false,
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap(),
    );
    assert!(matches!(
        service(&s, &f).execute(1, &request, || 11).await,
        Err(Error::MutationPending(_))
    ));
    let candidates = service(&s, &f).create_candidates(1, &id).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["id"], "new1");
    assert!(
        service(&s, &f)
            .adopt_create(1, &id, "existing", || 12)
            .await
            .is_err()
    );
    let i = service(&s, &f)
        .adopt_create(1, &id, "new1", || 13)
        .await
        .unwrap();
    assert_eq!(i.state, "applied");
    service(&s, &f).drive(1, &id, || 14).await.unwrap();
    assert_eq!(f.state.lock().await.writes, vec!["create"]);
    let folders = s.catalogue(1, Catalog::Folder).await.unwrap();
    assert_eq!(folders.as_array().unwrap().len(), 1);
    assert_eq!(folders[0]["is_pinned"], true);
    assert_eq!(folders[0]["sort_order"], 9);
}
#[tokio::test]
async fn paginated_membership_uncertainty_reconciles_without_clobbering_other_lists() {
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    for id in ["L1", "L2", "L3"] {
        f.list(id, id).await;
    }
    f.state
        .lock()
        .await
        .memberships
        .extend(["L1".into(), "L2".into()]);
    let star = seed_star(&s).await;
    let folder = seed_folder(&s, "L3").await;
    f.state.lock().await.drop_after = Some("membership".into());
    let request = Mutation::Membership {
        folder_id: folder,
        star_id: star,
        add: true,
    };
    let Error::MutationPending(id) = service(&s, &f)
        .execute(1, &request, || 10)
        .await
        .err()
        .unwrap()
    else {
        panic!()
    };
    let intent = service(&s, &f).reconcile(1, &id, || 11).await.unwrap();
    assert_eq!(intent.state, "applied");
    assert_eq!(f.state.lock().await.memberships.len(), 3);
    assert_eq!(f.state.lock().await.writes, vec!["membership"]);
    let summary: String = query("SELECT ai_summary FROM inbox_gh_stars WHERE id=?")
        .bind(star)
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get("ai_summary")
        .unwrap();
    assert_eq!(summary, "keep summary");
    let row =
        query("SELECT added_by FROM inbox_gh_folder_members WHERE folder_id=? AND gh_star_id=?")
            .bind(folder)
            .bind(star)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(row.try_get::<String, _>("added_by").unwrap(), "user");
    service(&s, &f)
        .execute(
            1,
            &Mutation::Membership {
                folder_id: folder,
                star_id: star,
                add: false,
            },
            || 12,
        )
        .await
        .unwrap();
    assert_eq!(f.state.lock().await.memberships.len(), 2);
    assert!(f.state.lock().await.memberships.contains("L1"));
    let folder = s.catalogue_one(1, Catalog::Folder, folder).await.unwrap();
    assert_eq!(folder["is_pinned"], true);
    assert_eq!(folder["sort_order"], 42);
    let state = f.state.lock().await;
    assert!(
        state
            .requests
            .iter()
            .any(|v| v["variables"]["after"] == "1")
    );
}
#[tokio::test]
async fn edit_and_delete_ambiguous_effects_reconcile_and_keep_manual_fields() {
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    f.list("L1", "Old").await;
    let folder = seed_folder(&s, "L1").await;
    f.state.lock().await.drop_after = Some("edit".into());
    let request = Mutation::EditList {
        folder_id: folder,
        body: json!({"name":"New"}),
    };
    let Error::MutationPending(id) = service(&s, &f)
        .execute(1, &request, || 10)
        .await
        .err()
        .unwrap()
    else {
        panic!()
    };
    service(&s, &f).reconcile(1, &id, || 11).await.unwrap();
    let row = s.catalogue_one(1, Catalog::Folder, folder).await.unwrap();
    assert_eq!(row["name"], "New");
    assert_eq!(row["sort_order"], 42);
    f.state.lock().await.drop_after = Some("delete".into());
    let Error::MutationPending(id) = service(&s, &f)
        .execute(1, &Mutation::DeleteList { folder_id: folder }, || 12)
        .await
        .err()
        .unwrap()
    else {
        panic!()
    };
    service(&s, &f).reconcile(1, &id, || 13).await.unwrap();
    assert!(s.catalogue_one(1, Catalog::Folder, folder).await.is_err());
    assert_eq!(f.state.lock().await.writes, vec!["edit", "delete"]);
}
#[tokio::test]
async fn process_dies_after_remote_acceptance_before_response() {
    let (d, s) = db().await;
    let f = std::sync::Arc::new(Fixture::new().await);
    f.list("L1", "Old").await;
    let folder = seed_folder(&s, "L1").await;
    f.state.lock().await.pause_after = Some("edit".into());
    let copy = s.clone();
    let fixture = f.clone();
    let task = tokio::spawn(async move {
        service(&copy, &fixture)
            .execute(
                1,
                &Mutation::EditList {
                    folder_id: folder,
                    body: json!({"name":"After crash"}),
                },
                || 10,
            )
            .await
    });
    f.reached.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let intent = s.active_mutations(1).await.unwrap().remove(0);
    assert_eq!(intent.state, "sending");
    s.db.close().await.unwrap();
    let s = Store::new(
        Database::open(
            &d.path().join("db"),
            false,
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap(),
    );
    service(&s, &f)
        .reconcile(1, &intent.id, || 11)
        .await
        .unwrap();
    f.release.notify_one();
    assert_eq!(f.state.lock().await.writes, vec!["edit"]);
    assert_eq!(
        s.catalogue_one(1, Catalog::Folder, folder).await.unwrap()["name"],
        "After crash"
    );
}
#[tokio::test]
async fn crash_after_confirm_before_atomic_apply_and_concurrent_send_cas() {
    let (d, s) = db().await;
    let f = Fixture::new().await;
    f.list("L1", "Old").await;
    let folder = seed_folder(&s, "L1").await;
    let request = Mutation::EditList {
        folder_id: folder,
        body: json!({"name":"Confirmed"}),
    };
    let i = s.prepare_mutation(1, &request, 10).await.unwrap();
    let p = i.prepared.clone().unwrap();
    assert!(s.begin_mutation_send(&i, &p, 11).await.unwrap());
    assert!(!s.begin_mutation_send(&i, &p, 11).await.unwrap());
    f.client
        .graphql(
            "fixture-secret",
            "mutation($input:UpdateUserListInput!){updateUserList(input:$input){list{id}}}",
            json!({"input":{"listId":"L1","name":"Confirmed"}}),
            true,
        )
        .await
        .unwrap();
    s.confirm_mutation(1, &i.id, &json!({}), 12).await.unwrap();
    s.db.close().await.unwrap();
    let s = Store::new(
        Database::open(
            &d.path().join("db"),
            false,
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap(),
    );
    service(&s, &f).drive(1, &i.id, || 13).await.unwrap();
    service(&s, &f).drive(1, &i.id, || 14).await.unwrap();
    assert_eq!(f.state.lock().await.writes.len(), 1);
}
#[tokio::test]
async fn prepared_crash_is_safe_to_send_and_unobserved_effect_stays_unknown() {
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    f.list("L1", "Old").await;
    let folder = seed_folder(&s, "L1").await;
    let request = Mutation::EditList {
        folder_id: folder,
        body: json!({"name":"Desired"}),
    };
    let i = s.prepare_mutation(1, &request, 1).await.unwrap();
    let c = service(&s, &f);
    let (a, b) = tokio::join!(c.drive(1, &i.id, || 2), c.drive(1, &i.id, || 2));
    assert!(a.is_ok() || b.is_ok());
    assert_eq!(f.state.lock().await.writes.len(), 1);
    let i = s
        .prepare_mutation(
            1,
            &Mutation::EditList {
                folder_id: folder,
                body: json!({"name":"Never sent"}),
            },
            3,
        )
        .await
        .unwrap();
    assert!(
        s.begin_mutation_send(&i, i.prepared.as_ref().unwrap(), 4)
            .await
            .unwrap()
    );
    assert!(matches!(
        c.drive(1, &i.id, || 5).await,
        Err(Error::MutationPending(_))
    ));
    assert!(s.cancel_prepared_mutation(1, &i.id, 6).await.is_err());
    assert_eq!(f.state.lock().await.writes.len(), 1);
    assert!(c.drive(3, &i.id, || 7).await.is_err());
}
#[tokio::test]
async fn star_uncertainty_uses_read_only_status_and_retains_ai() {
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    let star = seed_star(&s).await;
    f.state.lock().await.drop_after = Some("star".into());
    let Error::MutationPending(id) = service(&s, &f)
        .execute(
            1,
            &Mutation::Star {
                star_id: star,
                add: true,
            },
            || 10,
        )
        .await
        .err()
        .unwrap()
    else {
        panic!()
    };
    service(&s, &f).reconcile(1, &id, || 11).await.unwrap();
    let row = query("SELECT can_unstar,ai_summary FROM inbox_gh_stars WHERE id=?")
        .bind(star)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert!(row.try_get::<bool, _>("can_unstar").unwrap());
    assert_eq!(
        row.try_get::<String, _>("ai_summary").unwrap(),
        "keep summary"
    );
    assert_eq!(f.state.lock().await.writes, vec!["star"]);
}

#[tokio::test]
async fn definite_rejection_releases_admission_and_persists_no_credentials() {
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    f.state.lock().await.rejected = Some("create".into());
    let request = Mutation::CreateList {
        body: json!({"name":"Rejected"}),
    };
    assert!(matches!(
        service(&s, &f).execute(1, &request, || 10).await,
        Err(Error::Forbidden)
    ));
    assert!(s.active_mutations(1).await.unwrap().is_empty());
    assert!(f.state.lock().await.lists.is_empty());
    let row=query("SELECT state,request,prepared,result,local_folder_id FROM inbox_gh_mutations WHERE user_id=1").fetch_one(&s.db).await.unwrap();
    assert_eq!(row.try_get::<String, _>("state").unwrap(), "rejected");
    assert!(
        row.try_get::<Option<i64>, _>("local_folder_id")
            .unwrap()
            .is_none()
    );
    for key in ["request", "prepared", "result"] {
        let v: Option<serde_json::Value> = row.try_get(key).unwrap();
        assert!(!v.unwrap_or_default().to_string().contains("fixture-secret"));
    }
    let i = service(&s, &f).execute(1, &request, || 11).await.unwrap();
    assert_eq!(i.state, "applied");
    assert_eq!(f.state.lock().await.lists.len(), 1);
}
#[tokio::test]
async fn graphql_catalog_and_items_paginate_and_http_recovery_is_scoped() {
    use axum::{
        Extension,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    use yoink_inbox::http::{Actor, HttpState, router};
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    for id in ["L1", "L2", "L3"] {
        f.list(id, id).await;
    }
    f.state
        .lock()
        .await
        .memberships
        .extend(["L1".into(), "L2".into()]);
    let lists = yoink_inbox::lists::fetch(&f.client, "fixture-secret")
        .await
        .unwrap();
    assert_eq!(lists.len(), 3);
    assert_eq!(lists[0].items, vec![(10, "R1".into())]);
    assert!(
        f.state
            .lock()
            .await
            .requests
            .iter()
            .any(|v| v["variables"]["after"] == "2")
    );
    let state = HttpState {
        store: s.clone(),
        auth: Arc::new(Auth),
        insight: Arc::new(Keys),
        github: Arc::new(f.client.clone()),
    };
    let app = router(state).layer(Extension(Actor(1)));
    f.state.lock().await.drop_after = Some("create".into());
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/folders")
                .header("Idempotency-Key", "fixture-http-key")
                .header("Content-Type", "application/json")
                .body(Body::from("{\"name\":\"HTTP create\",\"is_local\":false}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    let b = to_bytes(r.into_body(), 65536).await.unwrap();
    assert!(!String::from_utf8_lossy(&b).contains("fixture-secret"));
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
    let id = v["mutation_id"].as_str().unwrap();
    let r = app
        .oneshot(
            Request::builder()
                .uri(format!("/gh_mutations/{id}/create-candidates"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(s.mutation(2, id).await.is_err());
}

#[tokio::test]
async fn applied_create_response_loss_replays_by_user_scoped_key() {
    use axum::{
        Extension,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    use yoink_inbox::http::{Actor, HttpState, router};
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    let app = router(HttpState {
        store: s.clone(),
        auth: Arc::new(Auth),
        insight: Arc::new(Keys),
        github: Arc::new(f.client.clone()),
    })
    .layer(Extension(Actor(1)));
    let request = |body: &'static str| {
        Request::builder()
            .method("POST")
            .uri("/folders")
            .header("Idempotency-Key", "fixture-http-key")
            .header("Content-Type", "application/json")
            .header("Idempotency-Key", "create-1")
            .body(Body::from(body))
            .unwrap()
    };
    let first = app
        .clone()
        .oneshot(request(
            r#"{"name":"Replay","is_pinned":true,"is_local":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    let first = to_bytes(first.into_body(), 65536).await.unwrap();
    let replay = app
        .clone()
        .oneshot(request(
            r#"{"name":"Replay","is_pinned":true,"is_local":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(first, to_bytes(replay.into_body(), 65536).await.unwrap());
    let mismatch = app
        .oneshot(request(r#"{"name":"Different","is_local":false}"#))
        .await
        .unwrap();
    assert_eq!(mismatch.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(f.state.lock().await.writes, vec!["create"]);
    let mutation = Mutation::CreateList {
        body: json!({"name":"Another user"}),
    };
    let other = service(&s, &f)
        .execute_keyed(2, &mutation, Some("create-1"), || 20)
        .await
        .unwrap();
    assert_eq!(other.state, "applied");
    assert_eq!(f.state.lock().await.writes, vec!["create", "create"]);
}

#[tokio::test]
async fn unknown_create_without_candidate_remains_blocked_but_original_local_actions_work() {
    use axum::{
        Extension,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    use yoink_inbox::http::{Actor, HttpState, router};
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    let request = Mutation::CreateList {
        body: json!({"name":"Unobservable"}),
    };
    let i = s.prepare_mutation(1, &request, 1).await.unwrap();
    s.begin_mutation_send(&i, &json!({"baseline":[]}), 2)
        .await
        .unwrap();
    let app = router(HttpState {
        store: s.clone(),
        auth: Arc::new(Auth),
        insight: Arc::new(Keys),
        github: Arc::new(f.client.clone()),
    })
    .layer(Extension(Actor(1)));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/gh_mutations/{}/create-candidates", i.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &to_bytes(response.into_body(), 65536).await.unwrap()
        )
        .unwrap(),
        json!([])
    );
    for action in ["cancel", "reconcile"] {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/gh_mutations/{}/{action}", i.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::CONFLICT);
    }
    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/gh_mutations/{}", i.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&to_bytes(status.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(v["can_cancel"], false);
    assert_eq!(v["can_inspect_candidates"], true);
    assert!(v.get("send_token").is_none());
    assert!(v["folder_id"].is_null());
    let local = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/folders")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"Still local"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(local.status(), StatusCode::CREATED);
    assert!(service(&s, &f).execute(1, &request, || 3).await.is_err());
    assert!(f.state.lock().await.writes.is_empty());
    assert_eq!(s.active_mutations(1).await.unwrap().len(), 1);
}
#[tokio::test]
async fn explicitly_adopted_create_reuses_candidate_imported_by_read_sync_preserving_manual_fields()
{
    let (_d, s) = db().await;
    let f = Fixture::new().await;
    f.state.lock().await.drop_after = Some("create".into());
    let err = service(&s, &f)
        .execute(
            1,
            &Mutation::CreateList {
                body: json!({"name":"Projects"}),
            },
            || 1,
        )
        .await
        .err()
        .unwrap();
    let Error::MutationPending(id) = err else {
        panic!("pending")
    };
    // Simulate the committed read-sync projection before explicit adoption.
    let imported = seed_folder(&s, "new1").await;
    let star = seed_star(&s).await;
    s.membership_confirmed(1, imported, star, true, 2)
        .await
        .unwrap();
    let i = service(&s, &f)
        .adopt_create(1, &id, "new1", || 3)
        .await
        .unwrap();
    assert_eq!(i.state, "applied");
    assert_eq!(i.local_folder_id, Some(imported));
    let folder = s.catalogue_one(1, Catalog::Folder, imported).await.unwrap();
    assert_eq!(folder["is_pinned"], true);
    assert_eq!(folder["sort_order"], 42);
    assert_eq!(folder["star_count"], 1);
    assert_eq!(
        s.catalogue(1, Catalog::Folder)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.state.lock().await.writes, vec!["create"]);
}
