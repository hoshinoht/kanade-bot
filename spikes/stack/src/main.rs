mod capabilities;
mod model;
mod store;
mod tls;

use model::fixture;
use sqlx::PgPool;
use std::{env, path::Path};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Chosen TLS backend is fixed here, before any rustls builder use, and
    // shared with tests through the same seam.
    tls::install_default_provider();
    match env::args().nth(1).as_deref() {
        Some("sqlite") => {
            sqlite_probe(
                env::args()
                    .nth(2)
                    .as_deref()
                    .unwrap_or("/tmp/kanade-stack.sqlite"),
            )
            .await?
        }
        Some("postgres") => {
            postgres_probe(&env::args().nth(2).ok_or("Postgres URL required")?).await?
        }
        Some("postgres-verify") => {
            postgres_verify(&env::args().nth(2).ok_or("Postgres URL required")?).await?
        }
        Some("capabilities") => capability_probe().await?,
        Some("clients") => client_probe().await?,
        Some("serve") => capabilities::serve().await?,
        _ => return Err(
            "usage: stack-spike {sqlite PATH|postgres URL|postgres-verify URL|capabilities|clients|serve}"
                .into(),
        ),
    }
    Ok(())
}

async fn sqlite_probe(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("sqlite://{}?mode=rwc", path);
    let pool = store::setup_sqlite(&url).await?;
    store::write_sqlite(&pool, &fixture()).await?;
    store::rollback_sqlite(&pool).await?;
    let rows = store::snapshot_sqlite(&pool).await?;
    assert_eq!(rows, fixture());
    let backup = format!("{path}.json");
    std::fs::write(&backup, serde_json::to_vec(&rows)?)?;
    let restored = format!("{path}.vacuum");
    sqlx::query(&format!("VACUUM INTO '{}'", restored.replace('\'', "''")))
        .execute(&pool)
        .await?;
    pool.close().await;
    let restored_pool = store::setup_sqlite(&format!("sqlite://{restored}?mode=rw")).await?;
    assert_eq!(store::snapshot_sqlite(&restored_pool).await?, fixture());
    println!(
        "sqlite rows={} backup={} restart=pass rollback=pass",
        rows.len(),
        backup
    );
    Ok(())
}

async fn postgres_probe(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = store::setup_postgres(url).await?;
    store::write_postgres(&pool, &fixture()).await?;
    store::rollback_postgres(&pool).await?;
    let rows = store::snapshot_postgres(&pool).await?;
    assert_eq!(rows, fixture());
    let backup = "/tmp/kanade-stack-postgres.json";
    std::fs::write(backup, serde_json::to_vec(&rows)?)?;
    pool.close().await;
    let restarted = store::setup_postgres(url).await?;
    assert_eq!(store::snapshot_postgres(&restarted).await?, fixture());
    store::clear_postgres(&restarted).await?;
    let imported: Vec<model::Delivery> = serde_json::from_slice(&std::fs::read(backup)?)?;
    store::write_postgres(&restarted, &imported).await?;
    assert_eq!(store::snapshot_postgres(&restarted).await?, fixture());
    println!(
        "postgres rows={} backup={} restart=pass rollback=pass",
        rows.len(),
        backup
    );
    Ok(())
}

async fn postgres_verify(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Read-only shape check of a restored database: no DDL, no writes committed.
    let pool = PgPool::connect(url).await?;
    let rows = store::snapshot_postgres(&pool).await?;
    assert_eq!(rows, fixture());
    store::rollback_postgres(&pool).await?;
    assert_eq!(store::snapshot_postgres(&pool).await?, fixture());
    pool.close().await;
    println!("postgres-verify rows={} rollback=pass", rows.len());
    Ok(())
}

async fn client_probe() -> Result<(), Box<dyn std::error::Error>> {
    // Production startup seam: the process default installed by application
    // bootstrap must exist before any outbound client is constructed.
    if tls::default_provider().is_none() {
        return Err("no default rustls crypto provider installed".into());
    }
    // Intended Discord HTTP client construction; no request is sent and no
    // token or gateway is contacted.
    let _discord = twilight_http::Client::new("synthetic-token".to_owned());
    // Selected SQLx TLS path authenticates through the same process default
    // via runtime-tokio-rustls; parse (never connect) representative options.
    let _options: sqlx::postgres::PgConnectOptions =
        "postgres://spike:synthetic-only@127.0.0.1:5432/kanade_spike".parse()?;
    println!("tls_provider=ring discord_http_client=constructed pg_options=parsed");
    Ok(())
}

async fn capability_probe() -> Result<(), Box<dyn std::error::Error>> {
    capabilities::http_asset_health_probe().await?;
    let timezone = capabilities::timezone_probe();
    assert_eq!(timezone, "2026-11-01T01:30:00-05:00");
    assert!(Path::new("fixtures/asset.txt").exists());
    println!("health=pass static_asset=pass timezone={timezone}");
    Ok(())
}
