use crate::database::models::{ModelAggregate, TimeBucket, TimeBucketAggregate, TokenUsage, UsageQueryOpts};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

pub struct TokenUsageRepository;

impl TokenUsageRepository {
    pub async fn record_usage(
        pool: &SqlitePool,
        usage: &TokenUsage,
    ) -> Result<i64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO token_usage (
                meeting_id, provider, model,
                prompt_tokens, completion_tokens, total_tokens,
                estimated_cost_usd, purpose, created_at, metadata
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&usage.meeting_id)
        .bind(&usage.provider)
        .bind(&usage.model)
        .bind(usage.prompt_tokens)
        .bind(usage.completion_tokens)
        .bind(usage.total_tokens)
        .bind(usage.estimated_cost_usd)
        .bind(&usage.purpose)
        .bind(usage.created_at)
        .bind(&usage.metadata)
        .execute(pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn list_usage(
        pool: &SqlitePool,
        opts: UsageQueryOpts,
    ) -> Result<Vec<TokenUsage>, sqlx::Error> {
        let limit = opts.limit.unwrap_or(500).clamp(1, 5000) as i64;

        let mut sql = String::from(
            "SELECT id, meeting_id, provider, model, \
             prompt_tokens, completion_tokens, total_tokens, \
             estimated_cost_usd, purpose, created_at, metadata \
             FROM token_usage WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();

        if let Some(provider) = &opts.provider {
            sql.push_str(" AND provider = ?");
            binds.push(provider.clone());
        }
        if let Some(model) = &opts.model {
            sql.push_str(" AND model = ?");
            binds.push(model.clone());
        }
        if let Some(purpose) = &opts.purpose {
            sql.push_str(" AND purpose = ?");
            binds.push(purpose.clone());
        }
        if let Some(meeting_id) = &opts.meeting_id {
            sql.push_str(" AND meeting_id = ?");
            binds.push(meeting_id.clone());
        }
        if let Some(since) = opts.since {
            sql.push_str(" AND created_at >= ?");
            binds.push(since.to_rfc3339());
        }
        if let Some(until) = opts.until {
            sql.push_str(" AND created_at <= ?");
            binds.push(until.to_rfc3339());
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?");

        let mut query = sqlx::query_as::<_, TokenUsage>(&sql);
        for b in &binds {
            query = query.bind(b);
        }
        query = query.bind(limit);
        query.fetch_all(pool).await
    }

    pub async fn aggregate_by_model(
        pool: &SqlitePool,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ModelAggregate>, sqlx::Error> {
        let mut sql = String::from(
            "SELECT provider, model, \
             COALESCE(SUM(prompt_tokens), 0) AS prompt_tokens, \
             COALESCE(SUM(completion_tokens), 0) AS completion_tokens, \
             COALESCE(SUM(total_tokens), 0) AS total_tokens, \
             COUNT(*) AS call_count \
             FROM token_usage WHERE 1=1",
        );
        if since.is_some() {
            sql.push_str(" AND created_at >= ?");
        }
        sql.push_str(" GROUP BY provider, model ORDER BY total_tokens DESC");

        let mut query = sqlx::query_as::<_, ModelAggregate>(&sql);
        if let Some(since) = since {
            query = query.bind(since);
        }
        query.fetch_all(pool).await
    }

    pub async fn aggregate_over_time(
        pool: &SqlitePool,
        bucket: TimeBucket,
        since: DateTime<Utc>,
    ) -> Result<Vec<TimeBucketAggregate>, sqlx::Error> {
        let format = match bucket {
            TimeBucket::Hour => "%Y-%m-%dT%H:00:00Z",
            TimeBucket::Day => "%Y-%m-%dT00:00:00Z",
            TimeBucket::Month => "%Y-%m-01T00:00:00Z",
        };

        let sql = format!(
            "SELECT \
             strftime('{fmt}', strftime('%Y-%m-%d %H:%M:%S', created_at)) AS bucket_start, \
             COALESCE(SUM(prompt_tokens), 0) AS prompt_tokens, \
             COALESCE(SUM(completion_tokens), 0) AS completion_tokens, \
             COALESCE(SUM(total_tokens), 0) AS total_tokens, \
             COUNT(*) AS call_count \
             FROM token_usage \
             WHERE created_at >= ? \
             GROUP BY bucket_start \
             ORDER BY bucket_start ASC",
            fmt = format
        );

        let rows = sqlx::query(&sql).bind(since).fetch_all(pool).await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let bucket_start: String = row.try_get("bucket_start")?;
            let bucket_start = DateTime::parse_from_rfc3339(&bucket_start)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
            out.push(TimeBucketAggregate {
                bucket_start,
                prompt_tokens: row.try_get("prompt_tokens")?,
                completion_tokens: row.try_get("completion_tokens")?,
                total_tokens: row.try_get("total_tokens")?,
                call_count: row.try_get("call_count")?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::TokenUsagePurpose;
    use crate::database::repositories::test_support::{insert_meeting, setup_pool};
    use chrono::TimeZone;

    fn usage(
        meeting_id: Option<&str>,
        provider: &str,
        model: &str,
        prompt: i64,
        completion: i64,
        purpose: TokenUsagePurpose,
        created_at: DateTime<Utc>,
    ) -> TokenUsage {
        TokenUsage {
            id: 0,
            meeting_id: meeting_id.map(|s| s.to_string()),
            provider: provider.to_string(),
            model: model.to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            estimated_cost_usd: None,
            purpose: purpose.into(),
            created_at,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn insert_and_get() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        let created_at = Utc.with_ymd_and_hms(2026, 8, 12, 10, 0, 0).unwrap();
        let row = usage(
            Some("m1"),
            "openai",
            "gpt-4o-mini",
            100,
            50,
            TokenUsagePurpose::SummaryChunk,
            created_at,
        );
        let id = TokenUsageRepository::record_usage(&pool, &row)
            .await
            .expect("insert failed");
        assert!(id > 0);

        let listed = TokenUsageRepository::list_usage(
            &pool,
            UsageQueryOpts {
                provider: Some("openai".into()),
                ..Default::default()
            },
        )
        .await
        .expect("list failed");

        assert_eq!(listed.len(), 1);
        let got = &listed[0];
        assert_eq!(got.id, id);
        assert_eq!(got.meeting_id.as_deref(), Some("m1"));
        assert_eq!(got.provider, "openai");
        assert_eq!(got.model, "gpt-4o-mini");
        assert_eq!(got.prompt_tokens, 100);
        assert_eq!(got.completion_tokens, 50);
        assert_eq!(got.total_tokens, 150);
        assert_eq!(got.purpose, "summary_chunk");
        assert_eq!(got.created_at, created_at);
    }

    #[tokio::test]
    async fn aggregate_by_model_basic() {
        let pool = setup_pool().await;
        let t0 = Utc.with_ymd_and_hms(2026, 8, 12, 9, 0, 0).unwrap();

        TokenUsageRepository::record_usage(
            &pool,
            &usage(
                None,
                "openai",
                "gpt-4o-mini",
                100,
                40,
                TokenUsagePurpose::SummaryChunk,
                t0,
            ),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(
                None,
                "openai",
                "gpt-4o-mini",
                50,
                25,
                TokenUsagePurpose::QaMeeting,
                t0,
            ),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(
                None,
                "claude",
                "claude-sonnet-4-5",
                200,
                100,
                TokenUsagePurpose::SummaryFinal,
                t0,
            ),
        )
        .await
        .unwrap();

        let rows = TokenUsageRepository::aggregate_by_model(&pool, None)
            .await
            .expect("aggregate failed");
        assert_eq!(rows.len(), 2);

        let gpt = rows
            .iter()
            .find(|r| r.model == "gpt-4o-mini")
            .expect("missing gpt row");
        assert_eq!(gpt.provider, "openai");
        assert_eq!(gpt.prompt_tokens, 150);
        assert_eq!(gpt.completion_tokens, 65);
        assert_eq!(gpt.total_tokens, 215);
        assert_eq!(gpt.call_count, 2);

        let claude = rows
            .iter()
            .find(|r| r.model == "claude-sonnet-4-5")
            .expect("missing claude row");
        assert_eq!(claude.prompt_tokens, 200);
        assert_eq!(claude.completion_tokens, 100);
        assert_eq!(claude.total_tokens, 300);
        assert_eq!(claude.call_count, 1);
    }

    #[tokio::test]
    async fn aggregate_over_time_day() {
        let pool = setup_pool().await;
        let day1_a = Utc.with_ymd_and_hms(2026, 8, 10, 10, 0, 0).unwrap();
        let day1_b = Utc.with_ymd_and_hms(2026, 8, 10, 23, 30, 0).unwrap();
        let day2 = Utc.with_ymd_and_hms(2026, 8, 11, 1, 15, 0).unwrap();

        TokenUsageRepository::record_usage(
            &pool,
            &usage(None, "openai", "gpt-4o-mini", 10, 5, TokenUsagePurpose::Other, day1_a),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(None, "openai", "gpt-4o-mini", 20, 8, TokenUsagePurpose::Other, day1_b),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(None, "openai", "gpt-4o-mini", 30, 12, TokenUsagePurpose::Other, day2),
        )
        .await
        .unwrap();

        let since = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let rows = TokenUsageRepository::aggregate_over_time(&pool, TimeBucket::Day, since)
            .await
            .expect("aggregate failed");
        assert_eq!(rows.len(), 2, "expected one bucket per calendar day");

        let expected_day1 = Utc.with_ymd_and_hms(2026, 8, 10, 0, 0, 0).unwrap();
        let expected_day2 = Utc.with_ymd_and_hms(2026, 8, 11, 0, 0, 0).unwrap();

        let d1 = rows
            .iter()
            .find(|r| r.bucket_start == expected_day1)
            .expect("missing day1");
        assert_eq!(d1.prompt_tokens, 30);
        assert_eq!(d1.completion_tokens, 13);
        assert_eq!(d1.total_tokens, 43);
        assert_eq!(d1.call_count, 2);

        let d2 = rows
            .iter()
            .find(|r| r.bucket_start == expected_day2)
            .expect("missing day2");
        assert_eq!(d2.prompt_tokens, 30);
        assert_eq!(d2.completion_tokens, 12);
        assert_eq!(d2.total_tokens, 42);
        assert_eq!(d2.call_count, 1);
    }

    #[tokio::test]
    async fn aggregate_over_time_hour() {
        let pool = setup_pool().await;
        let hour1 = Utc.with_ymd_and_hms(2026, 8, 10, 10, 5, 0).unwrap();
        let hour1_late = Utc.with_ymd_and_hms(2026, 8, 10, 10, 55, 0).unwrap();
        let hour2 = Utc.with_ymd_and_hms(2026, 8, 10, 11, 1, 0).unwrap();

        TokenUsageRepository::record_usage(
            &pool,
            &usage(None, "openai", "gpt-4o-mini", 10, 5, TokenUsagePurpose::Other, hour1),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(None, "openai", "gpt-4o-mini", 20, 8, TokenUsagePurpose::Other, hour1_late),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(None, "openai", "gpt-4o-mini", 30, 12, TokenUsagePurpose::Other, hour2),
        )
        .await
        .unwrap();

        let since = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let rows = TokenUsageRepository::aggregate_over_time(&pool, TimeBucket::Hour, since)
            .await
            .expect("aggregate failed");
        assert_eq!(rows.len(), 2, "expected one bucket per calendar hour");

        let expected_hour1 = Utc.with_ymd_and_hms(2026, 8, 10, 10, 0, 0).unwrap();
        let expected_hour2 = Utc.with_ymd_and_hms(2026, 8, 10, 11, 0, 0).unwrap();

        let h1 = rows
            .iter()
            .find(|r| r.bucket_start == expected_hour1)
            .expect("missing hour1");
        assert_eq!(h1.prompt_tokens, 30);
        assert_eq!(h1.completion_tokens, 13);
        assert_eq!(h1.total_tokens, 43);
        assert_eq!(h1.call_count, 2);

        let h2 = rows
            .iter()
            .find(|r| r.bucket_start == expected_hour2)
            .expect("missing hour2");
        assert_eq!(h2.prompt_tokens, 30);
        assert_eq!(h2.completion_tokens, 12);
        assert_eq!(h2.total_tokens, 42);
        assert_eq!(h2.call_count, 1);
    }

    #[tokio::test]
    async fn list_with_filters() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        insert_meeting(&pool, "m2").await;

        let t_early = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let t_mid = Utc.with_ymd_and_hms(2026, 8, 10, 0, 0, 0).unwrap();
        let t_late = Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).unwrap();

        TokenUsageRepository::record_usage(
            &pool,
            &usage(
                Some("m1"),
                "openai",
                "gpt-4o-mini",
                1,
                1,
                TokenUsagePurpose::QaMeeting,
                t_early,
            ),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(
                Some("m2"),
                "claude",
                "claude-sonnet-4-5",
                2,
                2,
                TokenUsagePurpose::SummaryFinal,
                t_mid,
            ),
        )
        .await
        .unwrap();
        TokenUsageRepository::record_usage(
            &pool,
            &usage(
                Some("m1"),
                "claude",
                "claude-sonnet-4-5",
                3,
                3,
                TokenUsagePurpose::SummaryChunk,
                t_late,
            ),
        )
        .await
        .unwrap();

        let since_t = Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap();
        let since_only = TokenUsageRepository::list_usage(
            &pool,
            UsageQueryOpts {
                since: Some(since_t),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(since_only.len(), 2);
        assert!(since_only.iter().all(|u| u.created_at >= since_t));

        let until_t = Utc.with_ymd_and_hms(2026, 8, 15, 0, 0, 0).unwrap();
        let windowed = TokenUsageRepository::list_usage(
            &pool,
            UsageQueryOpts {
                since: Some(since_t),
                until: Some(until_t),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(windowed.len(), 1);
        assert_eq!(windowed[0].model, "claude-sonnet-4-5");
        assert_eq!(windowed[0].created_at, t_mid);

        let by_provider = TokenUsageRepository::list_usage(
            &pool,
            UsageQueryOpts {
                provider: Some("claude".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(by_provider.len(), 2);
        assert!(by_provider.iter().all(|u| u.provider == "claude"));

        let by_meeting = TokenUsageRepository::list_usage(
            &pool,
            UsageQueryOpts {
                meeting_id: Some("m1".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(by_meeting.len(), 2);
        assert!(by_meeting.iter().all(|u| u.meeting_id.as_deref() == Some("m1")));

        let by_purpose = TokenUsageRepository::list_usage(
            &pool,
            UsageQueryOpts {
                purpose: Some("summary_final".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(by_purpose.len(), 1);
        assert_eq!(by_purpose[0].model, "claude-sonnet-4-5");

        let limited = TokenUsageRepository::list_usage(
            &pool,
            UsageQueryOpts {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].created_at, t_late, "should order by created_at DESC");
    }
}
