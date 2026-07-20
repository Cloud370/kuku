use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::tempdir;

use kuku::event::{TaskId, WorkspaceId};

use super::repository::TaskRepository;
use super::store::{
    CreateTaskCommand, ReviewSubmissionValidator, RunQueueAdmission, RunQueueReservation,
    SkillSelectionValidator, SubmitReviewCommand, SubmitRunCommand, TaskCommandService,
    ValidatedSkillSelection,
};

struct AllowSkills;
impl SkillSelectionValidator for AllowSkills {
    fn validate(
        &self,
        _: &WorkspaceId,
        tier_id: &str,
        skill_ids: &[String],
    ) -> Result<ValidatedSkillSelection, super::DomainError> {
        Ok(ValidatedSkillSelection {
            selection: kuku::event::SkillsChangedFact {
                tier_id: tier_id.to_owned(),
                skill_ids: skill_ids.to_vec(),
            },
        })
    }
}

struct RejectSkills;
impl SkillSelectionValidator for RejectSkills {
    fn validate(
        &self,
        _: &WorkspaceId,
        _: &str,
        _: &[String],
    ) -> Result<ValidatedSkillSelection, super::DomainError> {
        Err(super::DomainError::InvalidRequest)
    }
}

struct AllowReviews;
impl ReviewSubmissionValidator for AllowReviews {
    fn validate(
        &self,
        _: &TaskId,
        _: &kuku::event::ReviewSubmissionId,
        notes: &[kuku::event::ReviewAnnotationFact],
    ) -> Result<Vec<kuku::event::ReviewAnnotationFact>, super::DomainError> {
        Ok(notes.to_vec())
    }
}

struct RejectReviews;
impl ReviewSubmissionValidator for RejectReviews {
    fn validate(
        &self,
        _: &TaskId,
        _: &kuku::event::ReviewSubmissionId,
        _: &[kuku::event::ReviewAnnotationFact],
    ) -> Result<Vec<kuku::event::ReviewAnnotationFact>, super::DomainError> {
        Err(super::DomainError::InvalidRequest)
    }
}

struct BusyQueue;
impl RunQueueAdmission for BusyQueue {
    fn reserve(self: Arc<Self>) -> Result<Box<dyn RunQueueReservation>, super::DomainError> {
        Err(super::DomainError::ServerBusy)
    }

    fn ensure_admitted(
        &self,
        _: &TaskId,
        _: &kuku::event::RunId,
    ) -> Result<(), super::DomainError> {
        Ok(())
    }
}

#[derive(Default)]
struct CountingQueue {
    active: AtomicUsize,
    committed: AtomicUsize,
    admitted: Mutex<std::collections::HashSet<kuku::event::RunId>>,
}
impl RunQueueAdmission for CountingQueue {
    fn reserve(self: Arc<Self>) -> Result<Box<dyn RunQueueReservation>, super::DomainError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingReservation {
            queue: self.clone(),
            committed: false,
        }))
    }

    fn ensure_admitted(
        &self,
        _: &TaskId,
        run_id: &kuku::event::RunId,
    ) -> Result<(), super::DomainError> {
        self.admit(run_id);
        Ok(())
    }
}
impl CountingQueue {
    fn admit(&self, run_id: &kuku::event::RunId) {
        if self.admitted.lock().unwrap().insert(run_id.clone()) {
            self.committed.fetch_add(1, Ordering::SeqCst);
        }
    }
}
struct CountingReservation {
    queue: Arc<CountingQueue>,
    committed: bool,
}
impl RunQueueReservation for CountingReservation {
    fn commit(mut self: Box<Self>, _: TaskId, run_id: kuku::event::RunId) {
        self.committed = true;
        self.queue.admit(&run_id);
    }
}
impl Drop for CountingReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.queue.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[tokio::test]
async fn validators_and_queue_wrap_the_single_submission_transaction() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let queue = Arc::new(CountingQueue::default());
    let rejecting = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(RejectSkills),
        Arc::new(AllowReviews),
        queue.clone(),
    );
    let task = rejecting
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "ports-create".to_owned(),
        })
        .await
        .unwrap();
    let task_id = task.projection.task.task_id;
    let before = repository.replay(&task_id).unwrap().len();
    let submit = SubmitRunCommand {
        task_id: task_id.clone(),
        expected_task_revision: task.projection.task_revision,
        idempotency_key: "ports-reject".to_owned(),
        message: "hello".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    assert!(matches!(
        rejecting.submit(submit).await,
        Err(super::DomainError::InvalidRequest)
    ));
    assert_eq!(repository.replay(&task_id).unwrap().len(), before);
    assert_eq!(queue.active.load(Ordering::SeqCst), 0);

    let busy = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        Arc::new(BusyQueue),
    );
    let rejected = SubmitRunCommand {
        task_id: task_id.clone(),
        expected_task_revision: task.projection.task_revision,
        idempotency_key: "ports-busy".to_owned(),
        message: "hello".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    assert!(matches!(
        busy.submit(rejected).await,
        Err(super::DomainError::ServerBusy)
    ));
    assert_eq!(repository.replay(&task_id).unwrap().len(), before);

    let service = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        queue.clone(),
    );
    repository.fail_next_append_for_test();
    let failed = SubmitRunCommand {
        task_id: task_id.clone(),
        expected_task_revision: task.projection.task_revision,
        idempotency_key: "ports-append-fail".to_owned(),
        message: "hello".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    assert!(service.submit(failed).await.is_err());
    assert_eq!(queue.active.load(Ordering::SeqCst), 0);

    let second = service
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "review-create".to_owned(),
        })
        .await
        .unwrap();
    let review = SubmitReviewCommand {
        task_id: second.projection.task.task_id.clone(),
        expected_task_revision: second.projection.task_revision,
        idempotency_key: "review-submit".to_owned(),
        submission_id: kuku::event::ReviewSubmissionId::parse("rsub_1123456789abcdef01234567")
            .unwrap(),
        payload_hash: "sha256:review-payload".to_owned(),
        message: "Review notes".to_owned(),
        notes: Vec::new(),
    };
    let rejecting_review = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(AllowSkills),
        Arc::new(RejectReviews),
        queue.clone(),
    );
    let review_before = repository
        .replay(&second.projection.task.task_id)
        .unwrap()
        .len();
    assert!(matches!(
        rejecting_review.submit_review(review.clone()).await,
        Err(super::DomainError::InvalidRequest)
    ));
    assert_eq!(
        repository
            .replay(&second.projection.task.task_id)
            .unwrap()
            .len(),
        review_before
    );
    assert!(
        !service
            .submit_review(review.clone())
            .await
            .unwrap()
            .replayed
    );
    assert!(
        service
            .submit_review(review.clone())
            .await
            .unwrap()
            .replayed
    );
    let mut changed_content = review.clone();
    changed_content.message = "Different review notes".to_owned();
    assert!(matches!(
        service.submit_review(changed_content).await,
        Err(super::DomainError::IdempotencyConflict)
    ));
    let mut changed_review = review;
    changed_review.payload_hash = "sha256:changed-payload".to_owned();
    assert!(matches!(
        service.submit_review(changed_review).await,
        Err(super::DomainError::IdempotencyConflict)
    ));
    assert_eq!(queue.committed.load(Ordering::SeqCst), 1);
    let events = repository.replay(&second.projection.task.task_id).unwrap();
    let kuku::event::EventPayload::TaskLedger(kuku::event::TaskLedgerRecord::Control(transaction)) =
        &events.last().unwrap().payload
    else {
        panic!("control record")
    };
    assert!(transaction
        .events()
        .iter()
        .any(|event| matches!(event, kuku::event::TaskEvent::ReviewSubmissionRecorded(_))));
    assert!(transaction
        .events()
        .iter()
        .any(|event| matches!(event, kuku::event::TaskEvent::RunQueued { .. })));

    let uncertain = service
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "uncertain-queue-create".to_owned(),
        })
        .await
        .unwrap();
    let command = SubmitRunCommand {
        task_id: uncertain.projection.task.task_id,
        expected_task_revision: uncertain.projection.task_revision,
        idempotency_key: "uncertain-queue-submit".to_owned(),
        message: "hello".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    repository.fail_next_publication_for_test();
    assert!(service.submit(command.clone()).await.is_err());
    assert_eq!(queue.committed.load(Ordering::SeqCst), 2);
    assert!(service.submit(command).await.unwrap().replayed);
    assert_eq!(queue.committed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn durable_post_write_errors_repair_all_submission_receipts_in_process() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let queue = Arc::new(CountingQueue::default());
    let service = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        queue.clone(),
    );
    let workspace_id = WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap();
    let create = CreateTaskCommand {
        workspace_id: workspace_id.clone(),
        idempotency_key: "late-create".to_owned(),
    };
    repository.fail_next_store_after_write_for_test();
    repository.fail_durability_confirmations_for_test(1);
    assert!(service.create_task(create.clone()).await.is_err());
    let created = service.create_task(create).await.unwrap();
    assert!(created.replayed);

    let submit = SubmitRunCommand {
        task_id: created.projection.task.task_id,
        expected_task_revision: created.projection.task_revision,
        idempotency_key: "late-submit".to_owned(),
        message: "late submit".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    repository.fail_next_store_after_write_for_test();
    assert!(service.submit(submit.clone()).await.is_err());
    assert_eq!(queue.committed.load(Ordering::SeqCst), 1);
    assert!(service.submit(submit).await.unwrap().replayed);
    assert_eq!(queue.committed.load(Ordering::SeqCst), 1);

    let review_task = service
        .create_task(CreateTaskCommand {
            workspace_id,
            idempotency_key: "late-review-create".to_owned(),
        })
        .await
        .unwrap();
    let review = SubmitReviewCommand {
        task_id: review_task.projection.task.task_id,
        expected_task_revision: review_task.projection.task_revision,
        idempotency_key: "late-review".to_owned(),
        submission_id: kuku::event::ReviewSubmissionId::parse("rsub_2123456789abcdef01234567")
            .unwrap(),
        payload_hash: "sha256:late-review".to_owned(),
        message: "late review".to_owned(),
        notes: Vec::new(),
    };
    repository.fail_next_store_after_write_for_test();
    repository.fail_durability_confirmations_for_test(2);
    assert!(service.submit_review(review.clone()).await.is_err());
    assert_eq!(queue.committed.load(Ordering::SeqCst), 1);
    assert!(service.submit_review(review).await.unwrap().replayed);
    assert_eq!(queue.committed.load(Ordering::SeqCst), 2);

    let unconfirmed = service
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "unconfirmed-create".to_owned(),
        })
        .await
        .unwrap();
    let unconfirmed_submit = SubmitRunCommand {
        task_id: unconfirmed.projection.task.task_id,
        expected_task_revision: unconfirmed.projection.task_revision,
        idempotency_key: "unconfirmed-submit".to_owned(),
        message: "unconfirmed".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    repository.fail_next_store_after_write_for_test();
    repository.fail_durability_confirmations_for_test(2);
    assert!(service.submit(unconfirmed_submit.clone()).await.is_err());
    assert_eq!(queue.committed.load(Ordering::SeqCst), 2);
    assert!(
        service
            .submit(unconfirmed_submit.clone())
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(queue.committed.load(Ordering::SeqCst), 3);
    assert!(service.submit(unconfirmed_submit).await.unwrap().replayed);
    assert_eq!(queue.committed.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn oversized_submission_is_rejected_before_the_ledger_and_queue_commit() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let queue = Arc::new(CountingQueue::default());
    let service = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        queue.clone(),
    );
    let created = service
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "oversized-create".to_owned(),
        })
        .await
        .unwrap();
    let task_id = created.projection.task.task_id;
    let before = repository.replay(&task_id).unwrap().len();
    let result = service
        .submit(SubmitRunCommand {
            task_id: task_id.clone(),
            expected_task_revision: created.projection.task_revision,
            idempotency_key: "oversized-submit".to_owned(),
            message: "x".repeat(16 * 1024 * 1024 + 1),
            tier_id: "tier:default".to_owned(),
            skill_ids: Vec::new(),
        })
        .await;

    assert!(matches!(result, Err(super::DomainError::PayloadTooLarge)));
    assert_eq!(repository.replay(&task_id).unwrap().len(), before);
    assert_eq!(queue.active.load(Ordering::SeqCst), 0);
    assert_eq!(queue.committed.load(Ordering::SeqCst), 0);
    let review_before = repository.replay(&task_id).unwrap().len();
    let review = service
        .submit_review(SubmitReviewCommand {
            task_id: task_id.clone(),
            expected_task_revision: created.projection.task_revision,
            idempotency_key: "oversized-review".to_owned(),
            submission_id: kuku::event::ReviewSubmissionId::parse("rsub_3123456789abcdef01234567")
                .unwrap(),
            payload_hash: "sha256:oversized-review".to_owned(),
            message: "review".to_owned(),
            notes: vec![kuku::event::ReviewAnnotationFact {
                path: "src/lib.rs".to_owned(),
                revision: kuku::event::RevisionToken::parse("a".repeat(64)).unwrap(),
                side: kuku::event::AnnotationSide::New,
                start_line: 1,
                end_line: 1,
                excerpt: "line".to_owned(),
                comment: "x".repeat(16 * 1024 * 1024 + 1),
            }],
        })
        .await;
    assert!(matches!(review, Err(super::DomainError::PayloadTooLarge)));
    assert_eq!(repository.replay(&task_id).unwrap().len(), review_before);
    assert_eq!(queue.active.load(Ordering::SeqCst), 0);
    assert_eq!(queue.committed.load(Ordering::SeqCst), 0);
    drop(service);
    drop(repository);
    let reopened = TaskRepository::open(dir.path()).unwrap();
    assert_eq!(
        reopened.rebuild(&task_id).unwrap().revision(),
        created.projection.task_revision
    );
}

#[tokio::test]
async fn receipt_replay_repairs_only_queued_run_admission() {
    let dir = tempdir().unwrap();
    let repository = TaskRepository::open(dir.path()).unwrap();
    let queue = Arc::new(CountingQueue::default());
    let service = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        queue,
    );
    let created = service
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "state-replay-create".to_owned(),
        })
        .await
        .unwrap();
    let command = SubmitRunCommand {
        task_id: created.projection.task.task_id,
        expected_task_revision: created.projection.task_revision,
        idempotency_key: "state-replay-submit".to_owned(),
        message: "state replay".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    let submitted = service.submit(command.clone()).await.unwrap();
    repository
        .append(
            &submitted.task_id,
            kuku::event::TaskLedgerRecord::Activity(
                kuku::event::TaskActivityBatch::try_new(vec![kuku::event::TaskEvent::RunStarted {
                    run: kuku::event::RunFact {
                        run_id: submitted.run_id.clone(),
                        task_id: submitted.task_id.clone(),
                        state: kuku::event::RunState::Running,
                        started_at: "2026-07-20T00:00:00Z".to_owned(),
                        finished_at: None,
                        summary: None,
                        checks: None,
                        metrics: None,
                        workspace_changes: None,
                    },
                }])
                .unwrap(),
            ),
        )
        .unwrap();
    let running_queue = Arc::new(CountingQueue::default());
    let running = TaskCommandService::new_unchecked_with_ports(
        repository.clone(),
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        running_queue.clone(),
    );
    assert!(running.submit(command.clone()).await.unwrap().replayed);
    assert_eq!(running_queue.committed.load(Ordering::SeqCst), 0);

    repository
        .append(
            &submitted.task_id,
            kuku::event::TaskLedgerRecord::Activity(
                kuku::event::TaskActivityBatch::try_new(vec![
                    kuku::event::TaskEvent::RunCompleted {
                        run: kuku::event::RunFact {
                            run_id: submitted.run_id,
                            task_id: submitted.task_id.clone(),
                            state: kuku::event::RunState::Completed,
                            started_at: "2026-07-20T00:00:00Z".to_owned(),
                            finished_at: Some("2026-07-20T00:00:01Z".to_owned()),
                            summary: Some("done".to_owned()),
                            checks: None,
                            metrics: None,
                            workspace_changes: None,
                        },
                    },
                ])
                .unwrap(),
            ),
        )
        .unwrap();
    let reopened = TaskRepository::open(dir.path()).unwrap();
    let terminal_queue = Arc::new(CountingQueue::default());
    let terminal = TaskCommandService::new_unchecked_with_ports(
        reopened,
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        terminal_queue.clone(),
    );
    assert!(terminal.submit(command).await.unwrap().replayed);
    assert_eq!(terminal_queue.committed.load(Ordering::SeqCst), 0);

    let queued = terminal
        .create_task(CreateTaskCommand {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            idempotency_key: "queued-replay-create".to_owned(),
        })
        .await
        .unwrap();
    let queued_command = SubmitRunCommand {
        task_id: queued.projection.task.task_id,
        expected_task_revision: queued.projection.task_revision,
        idempotency_key: "queued-replay-submit".to_owned(),
        message: "queued replay".to_owned(),
        tier_id: "tier:default".to_owned(),
        skill_ids: Vec::new(),
    };
    terminal.submit(queued_command.clone()).await.unwrap();
    let recovery_queue = Arc::new(CountingQueue::default());
    let recovery = TaskCommandService::new_unchecked_with_ports(
        TaskRepository::open(dir.path()).unwrap(),
        Arc::new(AllowSkills),
        Arc::new(AllowReviews),
        recovery_queue.clone(),
    );
    assert!(
        recovery
            .submit(queued_command.clone())
            .await
            .unwrap()
            .replayed
    );
    assert!(recovery.submit(queued_command).await.unwrap().replayed);
    assert_eq!(recovery_queue.committed.load(Ordering::SeqCst), 1);
}
