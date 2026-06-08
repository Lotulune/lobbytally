use crate::admin::{
    AdminReviewAction, AdminTaskFailureItem, AdminTaskFailureSummary, AdminTaskKind,
    AdminTaskSummary,
};
use crate::public_catalog::{
    AdminReviewCandidate, DiscoveryHomeResponse, DiscoveryHomeSections, PageMeta,
    PublicGameAnalysis, PublicGameDetail, PublicGameListItem, PublicGamesPage,
};
use mpgs_core::models::PublicCatalogStatus;
use sqlx_core::migrate::{Migration, MigrationType, Migrator};
use sqlx_core::row::Row;
use sqlx_postgres::{PgPool, PgPoolOptions, Postgres};
use std::borrow::Cow;

const INITIAL_MIGRATION_SQL: &str = include_str!("../migrations/0001_public_catalog_ops.sql");
const AUDIT_EVENTS_MIGRATION_SQL: &str = include_str!("../migrations/0002_ops_audit_events.sql");
const ADMIN_REVIEW_NOTES_MIGRATION_SQL: &str =
    include_str!("../migrations/0003_admin_review_notes.sql");
const OPS_TASKS_MIGRATION_SQL: &str = include_str!("../migrations/0004_ops_tasks.sql");

#[derive(Debug, Clone)]
pub struct ServiceConfigState {
    pub active_config_version: Option<String>,
    pub pending_config_version: Option<String>,
    pub restart_required: bool,
    pub last_startup_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub event_type: String,
    pub actor: String,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminTaskControlState {
    pub recent_tasks: Vec<AdminTaskSummary>,
    pub failure_summary: AdminTaskFailureSummary,
    pub failures: Vec<AdminTaskFailureItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminOverviewStats {
    pub public_game_count: i64,
    pub pending_review_count: i64,
}

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx_core::error::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
}

pub fn migrator() -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![
            Migration::new(
                1,
                Cow::Borrowed("public_catalog_ops"),
                MigrationType::Simple,
                Cow::Borrowed(INITIAL_MIGRATION_SQL),
                false,
            ),
            Migration::new(
                2,
                Cow::Borrowed("ops_audit_events"),
                MigrationType::Simple,
                Cow::Borrowed(AUDIT_EVENTS_MIGRATION_SQL),
                false,
            ),
            Migration::new(
                3,
                Cow::Borrowed("admin_review_notes"),
                MigrationType::Simple,
                Cow::Borrowed(ADMIN_REVIEW_NOTES_MIGRATION_SQL),
                false,
            ),
            Migration::new(
                4,
                Cow::Borrowed("ops_tasks"),
                MigrationType::Simple,
                Cow::Borrowed(OPS_TASKS_MIGRATION_SQL),
                false,
            ),
        ]),
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
        r#"
        SELECT COUNT(*)
        FROM public_catalog.games
        WHERE review_status = 'accepted'
          AND visibility = 'public'
        "#,
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
        ) AND EXISTS (
            SELECT 1
            FROM _sqlx_migrations
            WHERE version = 2
              AND description = 'ops_audit_events'
              AND success = TRUE
        ) AND EXISTS (
            SELECT 1
            FROM _sqlx_migrations
            WHERE version = 3
              AND description = 'admin_review_notes'
              AND success = TRUE
        ) AND EXISTS (
            SELECT 1
            FROM _sqlx_migrations
            WHERE version = 4
              AND description = 'ops_tasks'
              AND success = TRUE
        )
        "#,
    )
    .fetch_one(pool)
    .await
}

pub async fn service_config_state(
    pool: &PgPool,
) -> Result<ServiceConfigState, sqlx_core::error::Error> {
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT active_config_version, pending_config_version, restart_required, last_startup_status
        FROM ops.service_config_state
        WHERE id = TRUE
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(ServiceConfigState {
        active_config_version: row.try_get("active_config_version")?,
        pending_config_version: row.try_get("pending_config_version")?,
        restart_required: row.try_get("restart_required")?,
        last_startup_status: row.try_get("last_startup_status")?,
    })
}

pub async fn record_active_config_startup(
    pool: &PgPool,
    active_config_version: &str,
) -> Result<(), sqlx_core::error::Error> {
    sqlx_core::query::query::<Postgres>(
        r#"
        UPDATE ops.service_config_state
        SET active_config_version = $1,
            pending_config_version = NULL,
            restart_required = FALSE,
            last_startup_status = 'ok',
            last_startup_at = now(),
            updated_at = now()
        WHERE id = TRUE
        "#,
    )
    .bind(active_config_version)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_pending_config(
    pool: &PgPool,
    pending_config_version: &str,
) -> Result<(), sqlx_core::error::Error> {
    sqlx_core::query::query::<Postgres>(
        r#"
        UPDATE ops.service_config_state
        SET pending_config_version = $1,
            restart_required = TRUE,
            updated_at = now()
        WHERE id = TRUE
        "#,
    )
    .bind(pending_config_version)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn record_audit_event(
    pool: &PgPool,
    event_type: &str,
    actor: &str,
    outcome: &str,
) -> Result<(), sqlx_core::error::Error> {
    sqlx_core::query::query::<Postgres>(
        r#"
        INSERT INTO ops.audit_events (event_type, actor, outcome)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(event_type)
    .bind(actor)
    .bind(outcome)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn latest_audit_event(
    pool: &PgPool,
) -> Result<Option<AuditEvent>, sqlx_core::error::Error> {
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT event_type, actor, outcome
        FROM ops.audit_events
        ORDER BY id DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?;

    row.map(|row| {
        Ok(AuditEvent {
            event_type: row.try_get("event_type")?,
            actor: row.try_get("actor")?,
            outcome: row.try_get("outcome")?,
        })
    })
    .transpose()
}

pub async fn recent_audit_events(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AuditEvent>, sqlx_core::error::Error> {
    let rows = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT event_type, actor, outcome
        FROM ops.audit_events
        ORDER BY id DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(AuditEvent {
                event_type: row.try_get("event_type")?,
                actor: row.try_get("actor")?,
                outcome: row.try_get("outcome")?,
            })
        })
        .collect()
}

pub async fn admin_overview_stats(
    pool: &PgPool,
) -> Result<AdminOverviewStats, sqlx_core::error::Error> {
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT
            COUNT(*) FILTER (
                WHERE review_status = 'accepted'
                  AND visibility = 'public'
            ) AS public_game_count,
            COUNT(*) FILTER (
                WHERE review_status = 'needs_review'
            ) AS pending_review_count
        FROM public_catalog.games
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(AdminOverviewStats {
        public_game_count: row.try_get("public_game_count")?,
        pending_review_count: row.try_get("pending_review_count")?,
    })
}

pub async fn admin_task_control_state(
    pool: &PgPool,
) -> Result<AdminTaskControlState, sqlx_core::error::Error> {
    let recent_tasks = recent_admin_tasks(pool, 6).await?;
    let failures = unresolved_task_failures(pool, 6).await?;
    let failure_summary = task_failure_summary(pool).await?;

    Ok(AdminTaskControlState {
        recent_tasks,
        failure_summary,
        failures,
    })
}

pub async fn create_admin_task(
    pool: &PgPool,
    task_type: AdminTaskKind,
    target_appid: Option<u32>,
) -> Result<AdminTaskSummary, sqlx_core::error::Error> {
    let target = target_appid.map(|appid| format!("appid:{appid}"));
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        INSERT INTO ops.tasks (task_type, target, target_appid, created_by)
        VALUES ($1, $2, $3, 'admin')
        RETURNING id, task_type, status, target, target_appid, created_at::text AS created_at, updated_at::text AS updated_at
        "#,
    )
    .bind(task_type.as_str())
    .bind(target)
    .bind(target_appid.map(|appid| appid as i32))
    .fetch_one(pool)
    .await?;

    admin_task_from_row(row)
}

async fn recent_admin_tasks(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AdminTaskSummary>, sqlx_core::error::Error> {
    let rows = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT id, task_type, status, target, target_appid, created_at::text AS created_at, updated_at::text AS updated_at
        FROM ops.tasks
        ORDER BY updated_at DESC, id DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(admin_task_from_row).collect()
}

async fn unresolved_task_failures(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AdminTaskFailureItem>, sqlx_core::error::Error> {
    let rows = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT task_id, stage, target, provider, retryable, attempt, reason, created_at::text AS created_at
        FROM ops.task_failures
        WHERE resolved_at IS NULL
        ORDER BY created_at DESC, id DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(admin_task_failure_from_row).collect()
}

async fn task_failure_summary(
    pool: &PgPool,
) -> Result<AdminTaskFailureSummary, sqlx_core::error::Error> {
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE resolved_at IS NULL) AS open_failure_count,
            COUNT(*) FILTER (WHERE resolved_at IS NULL AND retryable) AS retryable_failure_count
        FROM ops.task_failures
        "#,
    )
    .fetch_one(pool)
    .await?;
    let latest_failure = unresolved_task_failures(pool, 1).await?.into_iter().next();

    Ok(AdminTaskFailureSummary {
        open_failure_count: row.try_get("open_failure_count")?,
        retryable_failure_count: row.try_get("retryable_failure_count")?,
        latest_failure,
    })
}

pub async fn admin_review_queue(
    pool: &PgPool,
) -> Result<Vec<AdminReviewCandidate>, sqlx_core::error::Error> {
    let rows = sqlx_core::query::query::<Postgres>(
        r#"
        SELECT appid, name, review_status, visibility, recommendation_score, updated_at::text AS updated_at, review_note
        FROM public_catalog.games
        WHERE review_status = 'needs_review'
        ORDER BY updated_at DESC, appid ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(admin_review_candidate_from_row)
        .collect()
}

pub async fn apply_admin_review_action(
    pool: &PgPool,
    appid: u32,
    action: AdminReviewAction,
    note: Option<&str>,
) -> Result<Option<AdminReviewCandidate>, sqlx_core::error::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx_core::query::query::<Postgres>(
        r#"
        UPDATE public_catalog.games
        SET review_status = $2,
            visibility = $3,
            review_note = $4,
            updated_at = now()
        WHERE appid = $1
          AND review_status = 'needs_review'
        RETURNING appid, name, review_status, visibility, recommendation_score, updated_at::text AS updated_at, review_note
        "#,
    )
    .bind(appid as i32)
    .bind(action.review_status())
    .bind(action.visibility())
    .bind(note)
    .fetch_optional(&mut *tx)
    .await?;

    let candidate = row.map(admin_review_candidate_from_row).transpose()?;

    if candidate.is_some() && action.visibility() == "public" {
        sqlx_core::query::query::<Postgres>(
            r#"
            UPDATE public_catalog.public_catalog_state
            SET revision = revision + 1,
                status = 'ready',
                updated_at = now()
            WHERE id = TRUE
            "#,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(candidate)
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

fn admin_review_candidate_from_row(
    row: sqlx_postgres::PgRow,
) -> Result<AdminReviewCandidate, sqlx_core::error::Error> {
    let appid: i32 = row.try_get("appid")?;

    Ok(AdminReviewCandidate {
        appid: appid as u32,
        name: row.try_get("name")?,
        review_status: row.try_get("review_status")?,
        visibility: row.try_get("visibility")?,
        recommendation_score: row.try_get("recommendation_score")?,
        updated_at: row.try_get("updated_at")?,
        review_note: row.try_get("review_note")?,
    })
}

fn admin_task_from_row(
    row: sqlx_postgres::PgRow,
) -> Result<AdminTaskSummary, sqlx_core::error::Error> {
    let target_appid: Option<i32> = row.try_get("target_appid")?;

    Ok(AdminTaskSummary {
        id: row.try_get("id")?,
        task_type: row.try_get("task_type")?,
        status: row.try_get("status")?,
        target: row.try_get("target")?,
        target_appid: target_appid.map(|appid| appid as u32),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn admin_task_failure_from_row(
    row: sqlx_postgres::PgRow,
) -> Result<AdminTaskFailureItem, sqlx_core::error::Error> {
    Ok(AdminTaskFailureItem {
        task_id: row.try_get("task_id")?,
        stage: row.try_get("stage")?,
        target: row.try_get("target")?,
        provider: row.try_get("provider")?,
        retryable: row.try_get("retryable")?,
        attempt: row.try_get("attempt")?,
        reason: row.try_get("reason")?,
        created_at: row.try_get("created_at")?,
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
