use simple_blog::observability::{current_operation_id, operation, request_scope};

#[tokio::test]
async fn nested_operations_restore_the_parent_and_isolate_concurrent_requests() {
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let run = |root| {
        request_scope(root, async move {
            assert_eq!(current_operation_id(), Some(root));
            let child = operation("child", async {
                let id = current_operation_id().unwrap();
                tokio::task::yield_now().await;
                assert_eq!(current_operation_id(), Some(id));
                assert_ne!(id, root);
                Ok::<_, ()>(id)
            })
            .await
            .unwrap();
            assert_eq!(current_operation_id(), Some(root));
            child
        })
    };
    let (a, b) = tokio::join!(run(first), run(second));
    assert_ne!(a, b);
    assert_eq!(current_operation_id(), None);
}

#[tokio::test]
async fn failed_and_cancelled_operations_do_not_leak_identity_into_later_work() {
    let failed = operation("failure", async { Err::<(), _>("synthetic failure") }).await;
    assert!(failed.is_err());
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(10),
        operation("cancelled", std::future::pending::<Result<(), ()>>()),
    )
    .await;
    assert!(cancelled.is_err());
    assert_eq!(current_operation_id(), None);
}
