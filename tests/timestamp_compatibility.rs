use simple_blog::timestamp::Timestamp;
use sqlx::{Connection, SqliteConnection};
use time::SignedDuration;

#[test]
fn release_timestamps_require_a_complete_rfc3339_clock_and_offset() {
    for input in [
        "2026-01-01T+1:00:00Z",
        " 2026-01-01T01:00:00Z",
        "2026-01-01T01:00:00Z ",
        "2026-01-01T01:00:00.Z",
        "2026-01-01T01:00:00.123456789X",
        "2026-01-01T01:00:00+0900",
    ] {
        assert!(Timestamp::from_rfc3339(input).is_err(), "{input}");
    }
    for input in [
        "2026-01-01t01:00:00z",
        "2026-01-01T01:00:00.0000000001Z",
        "2026-01-01T00:00:00−01:00",
    ] {
        let utc = Timestamp::from_rfc3339(input).unwrap();
        assert_eq!(serde_json::to_value(utc).unwrap(), "2026-01-01T01:00:00Z");
    }
}

#[test]
fn arithmetic_preserves_the_inserted_second() {
    let leap: Timestamp = "2016-12-31T23:59:60.125Z".parse().unwrap();
    for (millis, expected) in [
        (-500, "2016-12-31T23:59:59.625Z"),
        (500, "2016-12-31T23:59:60.625Z"),
        (1000, "2017-01-01T00:00:00.125Z"),
    ] {
        let stamp = leap + SignedDuration::milliseconds(millis);
        assert_eq!(serde_json::to_value(stamp).unwrap(), expected);
    }
}

#[tokio::test]
async fn sqlite_rows_and_archive_values_keep_their_utc_spelling() {
    let mut database = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    for (input, json, row) in [
        (
            "2026-01-02T09:00:00+09:00",
            "2026-01-02T00:00:00Z",
            "2026-01-02T00:00:00+00:00",
        ),
        (
            "1969-12-31T23:59:59.1Z",
            "1969-12-31T23:59:59.100Z",
            "1969-12-31T23:59:59.100+00:00",
        ),
        (
            "2026-01-02T00:00:00.1234Z",
            "2026-01-02T00:00:00.123400Z",
            "2026-01-02T00:00:00.123400+00:00",
        ),
        (
            "2026-01-02T00:00:00.000000010Z",
            "2026-01-02T00:00:00.000000010Z",
            "2026-01-02T00:00:00.000000010+00:00",
        ),
        (
            "2016-12-31T23:59:60.125Z",
            "2016-12-31T23:59:60.125Z",
            "2016-12-31T23:59:60.125+00:00",
        ),
    ] {
        let stamp: Timestamp = serde_json::from_value(serde_json::json!(input)).unwrap();
        assert_eq!(serde_json::to_value(stamp).unwrap(), json);
        let written: String = sqlx::query_scalar("SELECT CAST(? AS TEXT)")
            .bind(stamp)
            .fetch_one(&mut database)
            .await
            .unwrap();
        assert_eq!(written, row);
        let read: Timestamp = sqlx::query_scalar("SELECT ?")
            .bind(row)
            .fetch_one(&mut database)
            .await
            .unwrap();
        assert_eq!(read, stamp);
    }
}

#[tokio::test]
async fn earlier_sqlite_values_decode_to_the_same_instant() {
    let mut database = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    let epoch: Timestamp = "1970-01-01T00:00:00Z".parse().unwrap();
    for query in [
        "SELECT '1970-01-01 00:00:00'",
        "SELECT '1970-01-01T01:00+01:00'",
        "SELECT 0",
        "SELECT 2440587.5",
    ] {
        let stamp: Timestamp = sqlx::query_scalar(query)
            .fetch_one(&mut database)
            .await
            .unwrap();
        assert_eq!(stamp, epoch);
    }
}
