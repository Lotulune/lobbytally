//! Account-only community aggregation. All public counts and previews join an
//! active account, deliberately excluding legacy anonymous votes.

use std::collections::HashMap;

use crate::pg::{Connection, params, params_from_iter};

use crate::error::StorageResult;
use crate::play_intent::{PlayIntentEpoch, epoch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunitySort {
    Trending,
    MostVoted,
}

impl CommunitySort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trending => "trending",
            Self::MostVoted => "most_voted",
        }
    }
}

/// Optional public community-list filters. Values are normalized by the HTTP
/// layer before reaching this query so every field remains a bound parameter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommunityFilters {
    pub release_state: Option<String>,
    pub demo_only: bool,
    pub platform: Option<String>,
    pub party_size: Option<u8>,
}

impl CommunityFilters {
    pub fn signature(&self) -> String {
        format!(
            "release={};demo={};platform={};party={}",
            self.release_state.as_deref().unwrap_or("any"),
            u8::from(self.demo_only),
            self.platform.as_deref().unwrap_or("any"),
            self.party_size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "any".to_owned()),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunityVoterPreview {
    pub display_name: String,
    pub avatar_public_id: String,
    pub avatar_version: u32,
    pub avatar_storage_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunityPlayIntentItem {
    pub app_id: u32,
    pub name: String,
    pub app_type: String,
    pub release_state: String,
    pub release_date: Option<String>,
    pub release_date_raw: Option<String>,
    pub release_date_precision: Option<String>,
    pub cover_url: Option<String>,
    pub cover_updated_at_ms: Option<i64>,
    pub count: u32,
    pub trending_count: u32,
    pub latest_vote_at_ms: i64,
    pub voted: bool,
    pub voters_preview: Vec<CommunityVoterPreview>,
    pub omitted_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunityPlayIntentPage {
    pub epoch: PlayIntentEpoch,
    pub items: Vec<CommunityPlayIntentItem>,
    pub has_more: bool,
}

pub fn page(
    conn: &Connection,
    current_user_id: Option<&str>,
    sort: CommunitySort,
    filters: &CommunityFilters,
    limit: usize,
    offset: usize,
    now_ms: i64,
) -> StorageResult<CommunityPlayIntentPage> {
    let tx = conn.read_transaction()?;
    let epoch = epoch(&tx)?;
    let trend_cutoff = now_ms.saturating_sub(7 * 24 * 60 * 60 * 1_000);
    let order = match sort {
        CommunitySort::Trending => {
            "votes.trending_count DESC, votes.total_count DESC, votes.latest_vote_at_ms DESC, apps.app_id ASC"
        }
        CommunitySort::MostVoted => {
            "votes.total_count DESC, votes.latest_vote_at_ms DESC, apps.app_id ASC"
        }
    };
    let query = format!(
        "WITH votes AS (
             SELECT v.app_id,
                    COUNT(*) AS total_count,
                    SUM(CASE WHEN v.created_at_ms >= $1 THEN 1 ELSE 0 END) AS trending_count,
                    MAX(v.created_at_ms) AS latest_vote_at_ms
             FROM play_intent_votes AS v
             JOIN user_accounts AS account ON account.user_id = v.user_id
             WHERE account.status = 'active'
             GROUP BY v.app_id
         )
         SELECT apps.app_id, apps.canonical_name, apps.app_type, apps.release_state,
                apps.release_date, apps.release_date_raw, apps.release_date_precision,
                media.capsule_url, media.updated_at_ms,
                votes.total_count, votes.trending_count, votes.latest_vote_at_ms,
                CASE WHEN $2::TEXT IS NULL THEN FALSE ELSE EXISTS (
                    SELECT 1 FROM play_intent_votes AS own_vote
                    JOIN user_accounts AS own_account ON own_account.user_id = own_vote.user_id
                    WHERE own_vote.app_id = apps.app_id AND own_vote.user_id = $2
                      AND own_account.status = 'active'
                ) END
         FROM votes
         JOIN apps ON apps.app_id = votes.app_id
         LEFT JOIN app_media AS media ON media.app_id = apps.app_id
         LEFT JOIN app_availability AS availability ON availability.app_id = apps.app_id
         LEFT JOIN multiplayer_profiles AS profile ON profile.app_id = apps.app_id
         WHERE ($3::TEXT IS NULL OR apps.release_state = $3)
           AND ($4 = 0 OR apps.app_type IN ('demo', 'playtest') OR EXISTS (
                SELECT 1 FROM app_relations AS demo_relation
                WHERE demo_relation.target_app_id = apps.app_id
                  AND demo_relation.relation_type IN ('demo_of', 'playtest_of')
           ))
           AND ($5::TEXT IS NULL OR EXISTS (
                SELECT 1 FROM jsonb_array_elements_text(COALESCE(availability.platforms_json, '[]')::JSONB) AS platform(value)
                WHERE lower(platform.value) = $5
           ))
           AND ($6::BIGINT IS NULL OR (
                profile.recommended_min_players IS NOT NULL
                AND profile.recommended_max_players IS NOT NULL
                AND profile.recommended_min_players <= $6
                AND profile.recommended_max_players >= $6
           ))
         ORDER BY {order}
         LIMIT $7 OFFSET $8"
    );
    let mut items = {
        let mut statement = tx.prepare(&query)?;
        let rows = statement.query_map(
            params![
                trend_cutoff,
                current_user_id,
                filters.release_state.as_deref(),
                i64::from(filters.demo_only),
                filters.platform.as_deref(),
                filters.party_size.map(i64::from),
                (limit.saturating_add(1)) as i64,
                offset as i64
            ],
            |row| {
                Ok(CommunityPlayIntentItem {
                    app_id: row.get::<_, i64>(0)? as u32,
                    name: row.get(1)?,
                    app_type: row.get(2)?,
                    release_state: row.get(3)?,
                    release_date: row.get(4)?,
                    release_date_raw: row.get(5)?,
                    release_date_precision: row.get(6)?,
                    cover_url: row.get(7)?,
                    cover_updated_at_ms: row.get(8)?,
                    count: row.get::<_, i64>(9)?.max(0) as u32,
                    trending_count: row.get::<_, i64>(10)?.max(0) as u32,
                    latest_vote_at_ms: row.get(11)?,
                    voted: row.get(12)?,
                    voters_preview: Vec::new(),
                    omitted_count: 0,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let has_more = items.len() > limit;
    items.truncate(limit);
    populate_voter_previews(&tx, &mut items, current_user_id)?;
    tx.commit()?;
    Ok(CommunityPlayIntentPage {
        epoch,
        items,
        has_more,
    })
}

pub fn previews_for_game(
    conn: &Connection,
    app_id: u32,
    current_user_id: Option<&str>,
) -> StorageResult<(PlayIntentEpoch, u32, bool, Vec<CommunityVoterPreview>, u32)> {
    let tx = conn.read_transaction()?;
    let epoch = epoch(&tx)?;
    let total: i64 = tx.query_row(
        "SELECT COUNT(*)
         FROM play_intent_votes AS v
         JOIN user_accounts AS account ON account.user_id = v.user_id
         WHERE v.app_id = $1 AND account.status = 'active'",
        params![app_id],
        |row| row.get(0),
    )?;
    let voted: bool = match current_user_id {
        Some(user_id) => tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM play_intent_votes AS v
                JOIN user_accounts AS account ON account.user_id = v.user_id
                WHERE v.app_id = $1 AND v.user_id = $2 AND account.status = 'active'
             )",
            params![app_id, user_id],
            |row| row.get(0),
        )?,
        None => false,
    };
    let mut item = CommunityPlayIntentItem {
        app_id,
        name: String::new(),
        app_type: String::new(),
        release_state: String::new(),
        release_date: None,
        release_date_raw: None,
        release_date_precision: None,
        cover_url: None,
        cover_updated_at_ms: None,
        count: total.max(0) as u32,
        trending_count: 0,
        latest_vote_at_ms: 0,
        voted,
        voters_preview: Vec::new(),
        omitted_count: 0,
    };
    populate_voter_previews(&tx, std::slice::from_mut(&mut item), current_user_id)?;
    tx.commit()?;
    Ok((
        epoch,
        item.count,
        item.voted,
        item.voters_preview,
        item.omitted_count,
    ))
}

fn populate_voter_previews(
    conn: &Connection,
    items: &mut [CommunityPlayIntentItem],
    current_user_id: Option<&str>,
) -> StorageResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let app_ids: Vec<u32> = items.iter().map(|item| item.app_id).collect();
    let markers = (2..=app_ids.len() + 1)
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "WITH ranked AS (
             SELECT v.app_id, account.display_name, account.avatar_public_id,
                    COALESCE(avatar.version, 0) AS avatar_version, avatar.storage_key,
                    ROW_NUMBER() OVER (
                        PARTITION BY v.app_id
                        ORDER BY CASE WHEN v.user_id = $1 THEN 0 ELSE 1 END,
                                 v.created_at_ms DESC, account.user_id ASC
                    ) AS row_number
             FROM play_intent_votes AS v
             JOIN user_accounts AS account ON account.user_id = v.user_id
             LEFT JOIN user_avatars AS avatar ON avatar.user_id = account.user_id
             WHERE account.status = 'active' AND v.app_id IN ({markers})
         )
         SELECT app_id, display_name, avatar_public_id, avatar_version, storage_key
         FROM ranked WHERE row_number <= 5
         ORDER BY app_id ASC, row_number ASC"
    );
    let mut values: Vec<crate::pg::types::Value> = Vec::with_capacity(app_ids.len() + 1);
    values.push(current_user_id.unwrap_or("").to_owned().into());
    values.extend(app_ids.iter().map(|id| i64::from(*id).into()));
    let mut statement = conn.prepare(&query)?;
    let rows = statement.query_map(params_from_iter(values.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)? as u32,
            CommunityVoterPreview {
                display_name: row.get(1)?,
                avatar_public_id: row.get(2)?,
                avatar_version: row.get::<_, i64>(3)?.max(0) as u32,
                avatar_storage_key: row.get(4)?,
            },
        ))
    })?;
    let mut by_app: HashMap<u32, Vec<CommunityVoterPreview>> = HashMap::new();
    for row in rows {
        let (app_id, preview) = row?;
        by_app.entry(app_id).or_default().push(preview);
    }
    for item in items {
        item.voters_preview = by_app.remove(&item.app_id).unwrap_or_default();
        item.omitted_count = item.count.saturating_sub(item.voters_preview.len() as u32);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Database, Repository,
        accounts::{RegisterAccount, register_account},
    };

    fn register(repo: &Repository, username: &str, now_ms: i64) -> String {
        repo.database()
            .with_conn_mut(|conn| {
                register_account(
                    conn,
                    &RegisterAccount {
                        username: username.into(),
                        display_name: username.into(),
                        password: format!("password-{username}-long"),
                        device_label: "test".into(),
                    },
                    None,
                    now_ms,
                )
            })
            .unwrap()
            .user_id
    }

    #[test]
    fn public_counts_exclude_anonymous_votes_and_preview_is_limited() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.database()
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT INTO apps (app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms)
                     VALUES (1, 'game', 'One', 'upcoming', 0, 0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let anonymous = repo.create_anonymous_session().unwrap();
        repo.database()
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT INTO play_intent_votes (app_id, user_id, created_at_ms) VALUES (1, $1, 1)",
                    params![anonymous.user_id],
                )?;
                Ok(())
            })
            .unwrap();
        for number in 0..6 {
            let user = register(&repo, &format!("player{number}"), 10 + number);
            repo.set_play_intent(&user, 1, true).unwrap();
        }
        let page = repo
            .database()
            .with_conn(|conn| {
                page(
                    conn,
                    None,
                    CommunitySort::MostVoted,
                    &CommunityFilters::default(),
                    10,
                    0,
                    100,
                )
            })
            .unwrap();
        assert_eq!(page.items[0].count, 6);
        assert_eq!(page.items[0].voters_preview.len(), 5);
        assert_eq!(page.items[0].omitted_count, 1);
    }

    #[test]
    fn metadata_filters_are_bound_and_limit_the_community_page() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.database()
            .with_conn_mut(|conn| {
                conn.execute_batch(
                    "INSERT INTO apps (app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms)
                       VALUES (1, 'game', 'Windows Demo', 'upcoming', 0, 0),
                              (2, 'game', 'Linux Release', 'released', 0, 0),
                              (1001, 'demo', 'Windows Demo Client', 'upcoming', 0, 0);
                     INSERT INTO app_availability (app_id, platforms_json, languages_json, updated_at_ms)
                       VALUES (1, '[\"windows\"]', '[]', 0),
                              (2, '[\"linux\"]', '[]', 0);
                     INSERT INTO multiplayer_profiles (
                       app_id, recommended_min_players, recommended_max_players, computed_at_ms
                     ) VALUES (1, 2, 4, 0), (2, 5, 8, 0);
                     INSERT INTO app_relations (source_app_id, target_app_id, relation_type, confidence, created_at_ms, updated_at_ms)
                       VALUES (1001, 1, 'demo_of', 1, 0, 0);",
                )?;
                Ok(())
            })
            .unwrap();
        for number in 0..2 {
            let user = register(&repo, &format!("filter{number}"), 10 + number);
            repo.set_play_intent(&user, number as u32 + 1, true)
                .unwrap();
        }
        let filters = CommunityFilters {
            release_state: Some("upcoming".into()),
            demo_only: true,
            platform: Some("windows".into()),
            party_size: Some(3),
        };
        let page = repo
            .database()
            .with_conn(|conn| page(conn, None, CommunitySort::Trending, &filters, 10, 0, 100))
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].app_id, 1);
        assert!(page.items[0].count > 0);
    }
}
