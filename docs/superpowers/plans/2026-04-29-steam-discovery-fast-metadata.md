# Steam Discovery Fast Metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Steam 游戏发现以更少请求、更准确评价统计导入元数据、评价好评度和当前游玩人数。

**Architecture:** 保持现有 `GameCard`、SQLite 表结构和发现任务控制台不变，把优化集中在 `src-tauri/src/steam.rs` 的 Steam API 查询策略。先用 appdetails 判定多人模式，非多人游戏不再抓评价和在线人数；确认多人后并发抓全语言评价汇总、本地化评价片段和当前玩家数。发现页大小默认值同步放宽，让每次 AppList 请求拿到更多候选。

**Tech Stack:** Tauri 2, Rust, reqwest, tokio `join!`, rusqlite, React 19, TypeScript, Vitest, Cargo tests

---

## Scope Check

本计划只覆盖 Steam 发现导入速度和数据准确性。以下内容不在本轮范围内：

- 新数据库字段或迁移
- Steam 用户资料、愿望单、好友数据
- AI 批量评估
- 全页面 UI 重设计
- 多 App 并发入库队列

## Evidence Notes

- Steamworks `IStoreService/GetAppList` 官方文档说明 `last_appid` 用于续页，`max_results` 默认 10k、最大 50k，并建议后续请求传入上一页最后一个 appid。
- Steamworks `appreviews/<appid>` 文档说明首个请求返回 `query_summary`，包含 `total_positive`、`total_negative`、`total_reviews`，`num_per_page` 最大 100。
- Steamworks `ISteamUserStats/GetNumberOfCurrentPlayers` 文档说明只需要 `appid`，返回当前连接 Steam 的活跃玩家数。
- `store.steampowered.com/api/appdetails` 是现有项目使用的公共 Store API；本轮继续使用它获取商店元数据，但在结论中保留“它不像 Web API 页面一样有完整 Steamworks 文档”的限制。

## File Structure

- Modify: `src-tauri/src/steam.rs`
  - 增加评价查询选项、全语言汇总计算、本地化片段查询、多人早过滤和并发 enrichment。
  - 增加私有单元测试，覆盖查询参数、好评率计算和多人 enrichment gate。
- Modify: `src-tauri/Cargo.toml`
  - 添加直接 `tokio` 依赖，用于 `tokio::join!` 宏。
- Modify: `src-tauri/tests/discovery_tests.rs`
  - 更新发现页大小默认值和上限断言。
- Modify: `src/features/discovery/DiscoveryTaskPanel.tsx`
  - 更新发现任务控制台默认每页数量、输入上限和前端 clamp。

## Task 1: Lock Review Accuracy Rules in Tests

**Files:**

- Modify: `src-tauri/src/steam.rs`

- [ ] **Step 1: Write failing tests for review query policy and metrics**

Add tests inside the existing `#[cfg(test)] mod tests` in `src-tauri/src/steam.rs`:

```rust
#[test]
fn review_summary_query_uses_all_languages_and_minimal_payload() {
    let query = review_summary_query();

    assert_eq!(query.filter, "all");
    assert_eq!(query.language, "all");
    assert_eq!(query.review_type, "all");
    assert_eq!(query.purchase_type, "all");
    assert_eq!(query.num_per_page, 1);
}

#[test]
fn review_metrics_use_global_positive_and_negative_totals() {
    let summary = ReviewFetch {
        total_reviews: Some(125),
        total_positive: Some(100),
        total_negative: Some(25),
        snippets: Vec::new(),
    };

    let (positive_pct, total_reviews) = review_metrics_from_summary(Some(&summary));

    assert_eq!(positive_pct, Some(80.0));
    assert_eq!(total_reviews, Some(125));
}

#[test]
fn review_metrics_fall_back_to_positive_plus_negative_total() {
    let summary = ReviewFetch {
        total_reviews: None,
        total_positive: Some(7),
        total_negative: Some(3),
        snippets: Vec::new(),
    };

    let (positive_pct, total_reviews) = review_metrics_from_summary(Some(&summary));

    assert_eq!(positive_pct, Some(70.0));
    assert_eq!(total_reviews, Some(10));
}
```

- [ ] **Step 2: Run the targeted failing tests**

Run:

```powershell
cargo test review_
```

Expected: tests fail because `ReviewQuery`, `review_summary_query`, `review_metrics_from_summary`, and `total_negative` do not exist yet.

## Task 2: Implement Review Summary and Snippet Queries

**Files:**

- Modify: `src-tauri/src/steam.rs`

- [ ] **Step 1: Add review query structures and metric helper**

Add this near the review fetch code in `src-tauri/src/steam.rs`:

```rust
#[derive(Debug, Clone, Copy)]
struct ReviewQuery<'a> {
    filter: &'a str,
    language: &'a str,
    review_type: &'a str,
    purchase_type: &'a str,
    num_per_page: u32,
}

fn review_summary_query() -> ReviewQuery<'static> {
    ReviewQuery {
        filter: "all",
        language: "all",
        review_type: "all",
        purchase_type: "all",
        num_per_page: 1,
    }
}

fn review_snippet_query(language: &str) -> ReviewQuery<'_> {
    ReviewQuery {
        filter: "recent",
        language,
        review_type: "all",
        purchase_type: "all",
        num_per_page: 10,
    }
}

fn review_metrics_from_summary(summary: Option<&ReviewFetch>) -> (Option<f64>, Option<u32>) {
    let Some(summary) = summary else {
        return (None, None);
    };

    let derived_total = match (summary.total_positive, summary.total_negative) {
        (Some(positive), Some(negative)) => positive.checked_add(negative),
        _ => None,
    };
    let total_reviews = summary.total_reviews.or(derived_total);
    let positive_review_pct = match (summary.total_positive, total_reviews) {
        (Some(positive), Some(total)) if total > 0 => {
            Some((positive as f64 / total as f64) * 100.0)
        }
        _ => None,
    };

    (positive_review_pct, total_reviews)
}
```

- [ ] **Step 2: Change `fetch_reviews` to accept `ReviewQuery`**

Replace the current `fetch_reviews` signature and query block with:

```rust
async fn fetch_reviews(client: &Client, appid: u32, query: ReviewQuery<'_>) -> Result<ReviewFetch> {
    let url = format!("https://store.steampowered.com/appreviews/{appid}");
    let response = client
        .get(url)
        .query(&[
            ("json", "1".to_string()),
            ("filter", query.filter.to_string()),
            ("language", query.language.to_string()),
            ("review_type", query.review_type.to_string()),
            ("purchase_type", query.purchase_type.to_string()),
            ("num_per_page", query.num_per_page.to_string()),
            ("cursor", "*".to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<AppReviewsResponse>()
        .await?;

    let snippets = response
        .reviews
        .unwrap_or_default()
        .into_iter()
        .filter_map(|review| {
            let text = review.review?;
            if text.trim().is_empty() {
                return None;
            }
            Some(ReviewSnippet {
                voted_up: review.voted_up.unwrap_or(false),
                review: normalize_review_text(&text),
                playtime_hours: review
                    .author
                    .and_then(|author| author.playtime_forever)
                    .map(|minutes| (minutes as f64 / 60.0 * 10.0).round() / 10.0),
            })
        })
        .collect();

    Ok(ReviewFetch {
        total_reviews: response.query_summary.total_reviews,
        total_positive: response.query_summary.total_positive,
        total_negative: response.query_summary.total_negative,
        snippets,
    })
}
```

- [ ] **Step 3: Extend private response structs**

Change `ReviewFetch` and `QuerySummary` to include `total_negative`:

```rust
struct ReviewFetch {
    total_reviews: Option<u32>,
    total_positive: Option<u32>,
    total_negative: Option<u32>,
    snippets: Vec<ReviewSnippet>,
}

struct QuerySummary {
    total_reviews: Option<u32>,
    total_positive: Option<u32>,
    total_negative: Option<u32>,
}
```

- [ ] **Step 4: Run the review tests**

Run:

```powershell
cargo test review_
```

Expected: all three tests pass.

## Task 3: Skip Expensive Enrichment for Non-Multiplayer Apps

**Files:**

- Modify: `src-tauri/src/steam.rs`

- [ ] **Step 1: Write failing enrichment gate tests**

Add tests inside `src-tauri/src/steam.rs`:

```rust
#[test]
fn app_details_without_multiplayer_modes_skip_expensive_enrichment() {
    let details = AppDetails {
        type_field: Some("game".to_string()),
        name: Some("Quiet Solo Game".to_string()),
        header_image: None,
        required_age: None,
        is_free: Some(false),
        supported_languages: None,
        price_overview: None,
        release_date: None,
        categories: Some(vec![StoreDescriptor {
            description: Some("Single-player".to_string()),
        }]),
        genres: None,
        demos: None,
        content_descriptors: None,
    };

    assert!(!should_fetch_review_and_player_enrichment(Some(&details)));
}

#[test]
fn app_details_with_multiplayer_modes_fetch_expensive_enrichment() {
    let details = AppDetails {
        type_field: Some("game".to_string()),
        name: Some("Co-op Signal".to_string()),
        header_image: None,
        required_age: None,
        is_free: Some(false),
        supported_languages: None,
        price_overview: None,
        release_date: None,
        categories: Some(vec![StoreDescriptor {
            description: Some("Online Co-op".to_string()),
        }]),
        genres: None,
        demos: None,
        content_descriptors: None,
    };

    assert!(should_fetch_review_and_player_enrichment(Some(&details)));
}
```

- [ ] **Step 2: Run the targeted failing tests**

Run:

```powershell
cargo test app_details_
```

Expected: tests fail because `should_fetch_review_and_player_enrichment` does not exist yet.

- [ ] **Step 3: Implement the enrichment gate**

Add this helper near `impl AppDetails`:

```rust
fn should_fetch_review_and_player_enrichment(details: Option<&AppDetails>) -> bool {
    details
        .map(AppDetails::multiplayer_modes)
        .is_some_and(|modes| !modes.is_empty())
}
```

- [ ] **Step 4: Run the enrichment gate tests**

Run:

```powershell
cargo test app_details_
```

Expected: both tests pass.

## Task 4: Stage and Parallelize Multiplayer Snapshot Fetching

**Files:**

- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/steam.rs`

- [ ] **Step 1: Add direct tokio macro dependency**

Add this dependency in `src-tauri/Cargo.toml`:

```toml
tokio = { version = "1", features = ["macros"] }
```

- [ ] **Step 2: Rewrite `fetch_game_snapshot` enrichment flow**

Change `fetch_game_snapshot` so it:

1. Fetches `appdetails` first.
2. Builds metadata from details immediately.
3. Returns without review/player enrichment when no multiplayer mode exists.
4. For multiplayer apps, runs summary, snippets, and current players concurrently:

```rust
let (review_summary, review_snippets, current_players) = if should_fetch_review_and_player_enrichment(details.as_ref()) {
    let (summary_result, snippets_result, players_result) = tokio::join!(
        fetch_reviews(client, appid, review_summary_query()),
        fetch_reviews(client, appid, review_snippet_query(language)),
        fetch_current_players(client, appid)
    );

    (
        summary_result.ok(),
        snippets_result.map(|reviews| reviews.snippets).unwrap_or_default(),
        players_result.ok(),
    )
} else {
    (None, Vec::new(), None)
};

let (positive_review_pct, total_reviews) = review_metrics_from_summary(review_summary.as_ref());
```

Keep existing metadata mapping for name, release date, languages, adult content, price, discount, capsule, tags, demo status, and multiplayer modes.

- [ ] **Step 3: Run all Steam unit tests**

Run:

```powershell
cargo test steam_
cargo test review_
cargo test app_details_
```

Expected: all listed tests pass.

## Task 5: Increase Discovery Candidate Page Size

**Files:**

- Modify: `src-tauri/src/discovery.rs`
- Modify: `src-tauri/tests/discovery_tests.rs`
- Modify: `src/features/discovery/DiscoveryTaskPanel.tsx`

- [ ] **Step 1: Write failing backend page-size test**

Change `discovery_defaults_scan_more_than_preview_page` in `src-tauri/tests/discovery_tests.rs`:

```rust
#[test]
fn discovery_defaults_scan_larger_candidate_pages() {
    assert_eq!(clamp_discovery_pages(None), 2);
    assert_eq!(clamp_discovery_page_size(None), 100);
    assert_eq!(clamp_discovery_pages(Some(99)), 5);
    assert_eq!(clamp_discovery_page_size(Some(999)), 250);
}
```

- [ ] **Step 2: Run the failing backend test**

Run:

```powershell
cargo test discovery_defaults_scan_larger_candidate_pages
```

Expected: fails because the current default is 25 and max is 50.

- [ ] **Step 3: Update backend clamp**

Change `src-tauri/src/discovery.rs`:

```rust
pub fn clamp_discovery_page_size(value: Option<u32>) -> u32 {
    value.unwrap_or(100).clamp(1, 250)
}
```

- [ ] **Step 4: Update frontend control defaults**

Change `src/features/discovery/DiscoveryTaskPanel.tsx`:

```tsx
const DEFAULT_PAGE_SIZE = 100;
const MAX_PAGE_SIZE = 250;
```

Use `MAX_PAGE_SIZE` for the page-size input `max` attribute and page-size `clamp`.

- [ ] **Step 5: Run backend and frontend targeted tests**

Run:

```powershell
cargo test discovery_defaults_scan_larger_candidate_pages
npm test -- DiscoveryTaskPanel
```

Expected: backend discovery test and discovery panel tests pass.

## Task 6: Final Verification

**Files:**

- No new files

- [ ] **Step 1: Run Rust tests for discovery and Steam metadata**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml discovery_tests
cargo test --manifest-path src-tauri/Cargo.toml steam
```

Expected: all targeted Rust tests pass.

- [ ] **Step 2: Run frontend discovery tests**

Run:

```powershell
npm test -- DiscoveryTaskPanel useDiscoveryTask
```

Expected: all targeted Vitest tests pass.

- [ ] **Step 3: Run full build checks**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml
npm test
npm run build
```

Expected: all tests and the TypeScript/Vite build pass.

## Self-Review

- Spec coverage: covers fast AppList candidate paging, accurate global review summary, localized review snippets, current player count, and avoiding unnecessary enrichment for non-multiplayer games.
- Placeholder scan: no `TBD`, no deferred tasks, no vague “add error handling later” step.
- Type consistency: all helper names used by later tasks are introduced before use: `ReviewQuery`, `review_summary_query`, `review_snippet_query`, `review_metrics_from_summary`, `should_fetch_review_and_player_enrichment`.
