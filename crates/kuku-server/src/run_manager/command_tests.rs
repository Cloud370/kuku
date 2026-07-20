use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
}

#[derive(Default)]
struct CountingQueue {
    active: AtomicUsize,
    committed: AtomicUsize,
}
impl RunQueueAdmission for CountingQueue {
    fn reserve(self: Arc<Self>) -> Result<Box<dyn RunQueueReservation>, super::DomainError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingReservation {
            queue: self.clone(),
            committed: false,
        }))
    }
}
struct CountingReservation {
    queue: Arc<CountingQueue>,
    committed: bool,
}
impl RunQueueReservation for CountingReservation {
    fn commit(mut self: Box<Self>, _: TaskId, _: kuku::event::RunId) {
        self.committed = true;
        self.queue.committed.fetch_add(1, Ordering::SeqCst);
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
