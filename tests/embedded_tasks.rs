mod embedded_support;

use chrono::{TimeZone, Utc};

#[tokio::test]
async fn embedded_create_task_and_ui_tasks() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, service) = embedded_support::setup_embedded_service().await?;

    let task = service
        .create_task(
            "Follow up with ACME",
            Some(Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap()),
        )
        .await?;

    assert_eq!(task["status"], "pending_confirmation");

    let tasks = service.ui_tasks().await?;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["title"], "Follow up with ACME");

    Ok(())
}
