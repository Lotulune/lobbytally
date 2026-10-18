use crate::pg::{Connection, params};

use crate::error::StorageResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityFinding {
    pub check_name: String,
    pub severity: String,
    pub app_id: Option<u32>,
    pub entity_key: Option<String>,
    pub message: String,
}

/// Run lightweight M2 data-quality checks and persist open findings.
pub fn run_quality_checks(conn: &Connection, now_ms: i64) -> StorageResult<Vec<QualityFinding>> {
    let mut findings = Vec::new();
    findings.extend(check_player_bounds(conn)?);
    findings.extend(check_relation_self_loops(conn)?);
    findings.extend(check_released_future_dates(conn, now_ms)?);
    findings.extend(check_active_override_without_app(conn)?);

    for finding in &findings {
        conn.execute(
            "INSERT INTO data_quality_findings (
                check_name, severity, app_id, entity_key, message, detected_at_ms
             ) VALUES ($1, $2, $3, $4, $5, $6)",
            params![
                finding.check_name,
                finding.severity,
                finding.app_id,
                finding.entity_key,
                finding.message,
                now_ms
            ],
        )?;
    }
    Ok(findings)
}

fn check_player_bounds(conn: &Connection) -> StorageResult<Vec<QualityFinding>> {
    let mut stmt = conn.prepare(
        "SELECT app_id, recommended_min_players, recommended_max_players
         FROM multiplayer_profiles
         WHERE recommended_min_players IS NOT NULL
           AND recommended_max_players IS NOT NULL
           AND recommended_min_players > recommended_max_players",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)? as u32,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (app_id, min_p, max_p) = row?;
        out.push(QualityFinding {
            check_name: "recommended_player_bounds".into(),
            severity: "error".into(),
            app_id: Some(app_id),
            entity_key: Some(app_id.to_string()),
            message: format!("recommended_min_players {min_p} > recommended_max_players {max_p}"),
        });
    }
    Ok(out)
}

fn check_relation_self_loops(conn: &Connection) -> StorageResult<Vec<QualityFinding>> {
    let mut stmt = conn.prepare(
        "SELECT source_app_id, target_app_id, relation_type
         FROM app_relations WHERE source_app_id = target_app_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)? as u32,
            row.get::<_, i64>(1)? as u32,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (source, target, rel) = row?;
        out.push(QualityFinding {
            check_name: "relation_self_loop".into(),
            severity: "error".into(),
            app_id: Some(source),
            entity_key: Some(format!("{source}->{target}:{rel}")),
            message: format!("relation {rel} points to itself"),
        });
    }
    Ok(out)
}

fn check_released_future_dates(
    conn: &Connection,
    now_ms: i64,
) -> StorageResult<Vec<QualityFinding>> {
    // Only flag ISO day precision dates clearly in the future while marked released.
    let today = crate::util::day_utc_from_ms(now_ms);
    let mut stmt = conn.prepare(
        "SELECT app_id, release_date FROM apps
         WHERE release_state = 'released'
           AND release_date IS NOT NULL
           AND length(release_date) = 10
           AND release_date > $1",
    )?;
    let rows = stmt.query_map(params![today], |row| {
        Ok((row.get::<_, i64>(0)? as u32, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (app_id, date) = row?;
        out.push(QualityFinding {
            check_name: "released_future_date".into(),
            severity: "warn".into(),
            app_id: Some(app_id),
            entity_key: Some(app_id.to_string()),
            message: format!("released app has future release_date {date}"),
        });
    }
    Ok(out)
}

fn check_active_override_without_app(conn: &Connection) -> StorageResult<Vec<QualityFinding>> {
    // Foreign keys should prevent this; kept as defensive check.
    let mut stmt = conn.prepare(
        "SELECT o.override_id, o.app_id FROM curation_overrides o
         LEFT JOIN apps a ON a.app_id = o.app_id
         WHERE o.revoked_at_ms IS NULL AND a.app_id IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u32))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (override_id, app_id) = row?;
        out.push(QualityFinding {
            check_name: "override_missing_app".into(),
            severity: "error".into(),
            app_id: Some(app_id),
            entity_key: Some(override_id.to_string()),
            message: "active override references missing app".into(),
        });
    }
    Ok(out)
}
