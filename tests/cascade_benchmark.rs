// Cascade timing benchmark against a live Postgres server.
//
// Ignored by default — run with:
//   LAZYDB_TEST_POSTGRES_URL=postgres://postgres@localhost:5432/cascade_test \
//     cargo test --test cascade_benchmark -- --ignored --nocapture
//
// Set SIMULATED_RTT_MS env var to add an artificial sleep before each query,
// to simulate a remote cloud DB where each round-trip costs N milliseconds.
//
// Measures the full fan-out pattern that `ExplorerTreeState::expand` +
// `accept_catalog_page` triggers after the user expands a database.

use std::time::{Duration, Instant};

use lazydb::{
    db::DatabaseConnection,
    db::catalog::{CatalogRequest, CatalogRequestKey, CatalogTarget, MAX_CATALOG_PAGE_SIZE},
    identity::ConnectionIdentity,
    profile::{CatalogScope, CatalogSelection, DatabaseKind, import_connection_url},
};
use uuid::Uuid;

const PAGE_SIZE: usize = 100;

fn make_request(
    profile_id: Uuid,
    target: CatalogTarget,
    cursor: Option<lazydb::db::catalog::CatalogCursor>,
) -> CatalogRequest {
    CatalogRequest {
        key: CatalogRequestKey {
            connection: ConnectionIdentity {
                profile_id,
                generation: 0,
            },
            catalog_epoch: 0,
            request_id: 0,
            target,
            cursor,
        },
        scope: CatalogScope::for_profile(DatabaseKind::Postgres, "cascade_test", None),
        page_size: PAGE_SIZE.min(MAX_CATALOG_PAGE_SIZE),
    }
}

async fn maybe_simulate_rtt() {
    if let Ok(ms_str) = std::env::var("SIMULATED_RTT_MS") {
        if let Ok(ms) = ms_str.parse::<u64>() {
            if ms > 0 {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
        }
    }
}

/// Simulates the OLD cascade behavior: opening the profile fires Schemas for
/// every database, then for every schema Groups, then for every group
/// Objects. This is the full eager fan-out the previous behavior performed.
async fn run_full_cascade(db: &DatabaseConnection, profile_id: Uuid) -> Duration {
    let started = Instant::now();

    maybe_simulate_rtt().await;
    let db_page = db
        .load_catalog_page(&make_request(profile_id, CatalogTarget::Databases, None))
        .await
        .expect("Databases");

    let schema_reqs: Vec<_> = db_page
        .entries
        .iter()
        .map(|e| {
            make_request(
                profile_id,
                CatalogTarget::schemas(e.id.clone()).unwrap(),
                None,
            )
        })
        .collect();
    let schema_pages: Vec<_> = futures_util::future::join_all(schema_reqs.iter().map(|r| async {
        maybe_simulate_rtt().await;
        db.load_catalog_page(r).await
    }))
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    let schema_ids: Vec<_> = schema_pages
        .iter()
        .flat_map(|p| p.entries.iter().map(|e| e.id.clone()))
        .collect();

    let group_reqs: Vec<_> = schema_ids
        .iter()
        .map(|sid| {
            make_request(
                profile_id,
                CatalogTarget::groups(sid.clone()).unwrap(),
                None,
            )
        })
        .collect();
    let group_pages: Vec<_> = futures_util::future::join_all(group_reqs.iter().map(|r| async {
        maybe_simulate_rtt().await;
        db.load_catalog_page(r).await
    }))
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();

    let mut in_flight: Vec<CatalogRequest> = Vec::new();
    for (i, gp) in group_pages.iter().enumerate() {
        let schema_id = &schema_ids[i];
        for summary in &gp.group_summaries {
            in_flight.push(make_request(
                profile_id,
                CatalogTarget::objects(schema_id.clone(), summary.group).unwrap(),
                None,
            ));
        }
    }
    let first_page_count = in_flight.len();
    let mut total_object_entries = 0usize;
    let mut total_pages = 0usize;
    while !in_flight.is_empty() {
        let pages: Vec<_> = futures_util::future::join_all(in_flight.iter().map(|r| async {
            maybe_simulate_rtt().await;
            db.load_catalog_page(r).await
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        let mut next: Vec<CatalogRequest> = Vec::new();
        for (i, p) in pages.iter().enumerate() {
            total_object_entries += p.entries.len();
            total_pages += 1;
            if let Some(c) = p.next_cursor.clone() {
                let mut r = in_flight[i].clone();
                r.key.cursor = Some(c);
                next.push(r);
            }
        }
        in_flight = next;
    }

    let elapsed = started.elapsed();
    eprintln!(
        "  full cascade wall time: {:?} | first_page_object_requests={} total_object_pages={} total_object_entries={} per_db_first_page={}",
        elapsed,
        first_page_count,
        total_pages,
        total_object_entries,
        first_page_count / db_page.entries.len().max(1),
    );
    elapsed
}

/// Simulates the NEW (lazy) cascade: only one schema + one group's data are
/// loaded, matching a user opening one schema and one group.
async fn run_user_opens_one_schema(db: &DatabaseConnection, profile_id: Uuid) -> Duration {
    let started = Instant::now();

    maybe_simulate_rtt().await;
    let db_page = db
        .load_catalog_page(&make_request(profile_id, CatalogTarget::Databases, None))
        .await
        .expect("Databases");

    let schema_reqs: Vec<_> = db_page
        .entries
        .iter()
        .map(|e| {
            make_request(
                profile_id,
                CatalogTarget::schemas(e.id.clone()).unwrap(),
                None,
            )
        })
        .collect();
    let schema_pages: Vec<_> = futures_util::future::join_all(schema_reqs.iter().map(|r| async {
        maybe_simulate_rtt().await;
        db.load_catalog_page(r).await
    }))
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    let schema_id = schema_pages
        .iter()
        .flat_map(|p| p.entries.iter().map(|e| e.id.clone()))
        .next()
        .expect("at least one schema");

    let group_req = make_request(
        profile_id,
        CatalogTarget::groups(schema_id.clone()).unwrap(),
        None,
    );
    let _group_page = db.load_catalog_page(&group_req).await.expect("Groups");

    // User opens one group: Tables
    let object_req = make_request(
        profile_id,
        CatalogTarget::objects(schema_id, lazydb::db::catalog::ObjectGroup::Tables).unwrap(),
        None,
    );
    let _objects_page = db.load_catalog_page(&object_req).await.expect("Objects");

    started.elapsed()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "requires live Postgres; set LAZYDB_TEST_POSTGRES_URL"]
async fn cascade_benchmark_against_live_postgres() {
    let url = match std::env::var("LAZYDB_TEST_POSTGRES_URL") {
        Ok(s) => s,
        Err(_) => {
            eprintln!("skipping: set LAZYDB_TEST_POSTGRES_URL");
            return;
        }
    };
    let imported = import_connection_url(&url, Some("cascade-bench"))
        .unwrap_or_else(|_| panic!("invalid LAZYDB_TEST_POSTGRES_URL: {url}"));
    let mut profile = imported.profile;
    profile.catalog_scope = CatalogScope::for_profile(
        profile.kind,
        profile.database.as_deref().unwrap_or(""),
        None,
    );
    profile.catalog_scope.databases = CatalogSelection::All;
    let profile_id = profile.id;

    let db = DatabaseConnection::connect(&profile, imported.transient_password.as_ref())
        .await
        .expect("connect failed");

    eprintln!("== cascade benchmark ==");
    eprintln!(
        "  simulated RTT (ms): {}",
        std::env::var("SIMULATED_RTT_MS").unwrap_or_default()
    );
    eprintln!("  pool size:         compiled-in (see src/db/postgres.rs)");
    eprintln!();

    eprintln!("-- OLD eager cascade: load everything --");
    let _ = run_full_cascade(&db, profile_id).await;
    let t_full_1 = run_full_cascade(&db, profile_id).await;
    let t_full_2 = run_full_cascade(&db, profile_id).await;

    eprintln!();
    eprintln!("-- NEW lazy: user opens one schema + one group --");
    let _ = run_user_opens_one_schema(&db, profile_id).await;
    let t_one_1 = run_user_opens_one_schema(&db, profile_id).await;
    let t_one_2 = run_user_opens_one_schema(&db, profile_id).await;

    eprintln!();
    eprintln!("== summary ==");
    eprintln!(
        "  full cascade (everything):   {:?} / {:?}",
        t_full_1, t_full_2
    );
    eprintln!("  one schema + one group:      {:?}", t_one_1);
    eprintln!("  one schema + one group (2nd): {:?}", t_one_2);
}
