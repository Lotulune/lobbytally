use crate::public_catalog::{
    DiscoveryHomeResponse, DiscoveryHomeSections, PageMeta, PublicGameAnalysis, PublicGameDetail,
    PublicGameListItem, PublicGamesPage,
};
use mpgs_core::models::PublicCatalogStatus;
use sqlx_core::migrate::{Migration, MigrationType, Migrator};
use sqlx_core::row::Row;
use sqlx_postgres::{PgPool, PgPoolOptions, Postgres};
use std::borrow::Cow;

const INITIAL_MIGRATION_SQL: &str = include_str!("../migrations/0001_public_catalog_ops.sql");

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx_core::error::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
}

pub fn migrator() -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![Migration::new(
            1,
            Cow::Borrowed("public_catalog_ops"),
            MigrationType::Simple,
            Cow::Borrowed(INITIAL_MIGRATION_SQL),
            false,
        )]),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    }
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx_core::migrate::MigrateError> {
    migrator().run(pool).await
}

pub async fn connect_and_migrate(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = connect(database_url).await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

pub async fn public_catalog_status(
    pool: &PgPool,
) -> Result<PublicCatalogStatus, sqlx_core::error::Error> {
    let public_game_count: i64 = sqlx_core::query_scalar::query_scalar::<Postgres, i64>(
        "SELECT COUNT(*) FROM public_catalog.games",
    )
    .fetch_one(pool)
    .await?;

    if public_game_count == 0 {
        Ok(PublicCatalogStatus::Empty)
    } else {
        Ok(PublicCatalogStatus::Ready)
    }
}

pub async fn public_catalog_revision(pool: &PgPool) -> Result<i64, sqlx_core::error::Error> {
    sqlx_core::query_scalar::query_scalar::<Postgres, i64>(
        "SELECT revision FROM public_catalog.public_catalog_state WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await
}

pub async fn migration_health_check(pool: &PgPool) -> Result<bool, sqlx_core::error::Error> {
    sqlx_core::query_scalar::query_scalar::<Postgres, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM _sqlx_migrations
            WHERE version = 1
              AND description = 'public_catalog_ops'
              AND success = TRUE
        )
        "#,
    )
    .fetch_one(pool)
    .await
}

pub async fn discovery_home(
    pool: &PgPool,
) -> Result<DiscoveryHomeResponse, sqlx_core::error::Error> {
    let total_games = public_games_count(pool).await?;

    if total_games == 0 {
        return Ok(DiscoveryHomeResponse::empty());
    }

    let newly_published = public_games_list(pool, 6, 0).await?;
    let high_confidence = public_games_by_score(pool, 6).await?;
    let recently_added = public_games_list(pool, 6, 0).await?;

    Ok(DiscoveryHomeResponse {
        status: PublicCatalogStatus::Ready,
        total_games,
        sections: DiscoveryHomeSections {
            newly_published,
            high_confidence,
            recently_added,
        },
    })
}

pub async fn public_games_page(
    pool: &PgPool,
    limit: u32,
    offset: u32,
) -> Result<PublicGamesPage, sqlx_core::error::Error> {
    let total = public_games_count(pool).await?;
    let items = public_games_list(pool, limit, offset).await?;

    Ok(PublicGamesPage {
        items,
        page: PageMeta {
            limit,
            offset,
            total,
        },
    })
}

pub async fn public_game_detail(
    pool: &PgPool,
    appid: u32,
) -> Result<Option<PublicGameDetail>, sqlx_core::error::Error> {
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT appid, name, recommendation_score, updated_at::text AS updated_at
        FROM public_catalog.games
        WHERE appid = $1
          AND review_status = 'accepted'
          AND visibility = 'public'
        "#,
    )
    .bind(appid as i32)
    .fetch_optional(pool)
    .await?;

    row.map(public_game_list_item_from_row)
        .transpose()
        .map(|item| item.map(|game| PublicGameDetail { game }))
}

pub async fn public_game_analysis(
    pool: &PgPool,
    appid: u32,
) -> Result<Option<PublicGameAnalysis>, sqlx_core::error::Error> {
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT analysis.appid, analysis.report_json, analysis.generated_at::text AS generated_at
        FROM public_catalog.game_analysis analysis
        JOIN public_catalog.games games ON games.appid = analysis.appid
        WHERE analysis.appid = $1
          AND games.review_status = 'accepted'
          AND games.visibility = 'public'
        "#,
    )
    .bind(appid as i32)
    .fetch_optional(pool)
    .await?;

    row.map(|row| {
        let appid: i32 = row.try_get("appid")?;
        Ok(PublicGameAnalysis {
            appid: appid as u32,
            report: row.try_get("report_json")?,
            generated_at: row.try_get("generated_at")?,
        })
    })
    .transpose()
}

async fn public_games_count(pool: &PgPool) -> Result<i64, sqlx_core::error::Error> {
    sqlx_core::query_scalar::query_scalar::<Postgres, i64>(
        r#"
        SELECT COUNT(*)
        FROM public_catalog.games
        WHERE review_status = 'accepted'
          AND visibility = 'public'
        "#,
    )
    .fetch_one(pool)
    .await
}

async fn public_games_list(
    pool: &PgPool,
    limit: u32,
    offset: u32,
) -> Result<Vec<PublicGameListItem>, sqlx_core::error::Error> {
    let rows = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT appid, name, recommendation_score, updated_at::text AS updated_at
        FROM public_catalog.games
        WHERE review_status = 'accepted'
          AND visibility = 'public'
        ORDER BY updated_at DESC, appid ASC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(public_game_list_item_from_row)
        .collect()
}

async fn public_games_by_score(
    pool: &PgPool,
    limit: u32,
) -> Result<Vec<PublicGameListItem>, sqlx_core::error::Error> {
    let rows = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT appid, name, recommendation_score, updated_at::text AS updated_at
        FROM public_catalog.games
        WHERE review_status = 'accepted'
          AND visibility = 'public'
        ORDER BY recommendation_score DESC NULLS LAST, updated_at DESC, appid ASC
        LIMIT $1
        "#,
    )
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(public_game_list_item_from_row)
        .collect()
}

fn public_game_list_item_from_row(
    row: sqlx_postgres::PgRow,
) -> Result<PublicGameListItem, sqlx_core::error::Error> {
    let appid: i32 = row.try_get("appid")?;

    Ok(PublicGameListItem {
        appid: appid as u32,
        name: row.try_get("name")?,
        recommendation_score: row.try_get("recommendation_score")?,
        updated_at: row.try_get("updated_at")?,
    })
}
