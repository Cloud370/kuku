//! Projects durable review submissions against current workspace anchors.

use std::collections::HashMap;
use std::sync::Arc;

use crate::api::{
    AnnotationSide, AnnotationStatus, ApiError, ApiErrorCode, PageCursor, ReviewSubmissionPage,
    TaskId,
};

use super::files::WorkspaceReadService;
use super::git::GitReviewService;
use super::{ReviewAdmission, ReviewLimits, ReviewRuntimePort, WorkspaceCapabilityProvider};

const TRACE_ID: &str = "review-submissions";

/// Pages durable submissions and derives their current anchor status.
pub struct ReviewSubmissionService {
    workspaces: Arc<dyn WorkspaceCapabilityProvider>,
    runtime: Arc<dyn ReviewRuntimePort>,
    files: Arc<WorkspaceReadService>,
    admission: Arc<ReviewAdmission>,
    limits: ReviewLimits,
}

impl ReviewSubmissionService {
    /// Creates a durable submission projection service.
    pub fn new(
        workspaces: Arc<dyn WorkspaceCapabilityProvider>,
        runtime: Arc<dyn ReviewRuntimePort>,
        limits: ReviewLimits,
    ) -> Self {
        let admission = Arc::new(ReviewAdmission::new(&limits));
        let files = Arc::new(WorkspaceReadService::with_admission(
            workspaces.clone(),
            limits.clone(),
            admission.clone(),
        ));
        Self::with_services(workspaces, runtime, files, admission, limits)
    }

    /// Creates a submission service using shared file and admission services.
    pub fn with_services(
        workspaces: Arc<dyn WorkspaceCapabilityProvider>,
        runtime: Arc<dyn ReviewRuntimePort>,
        files: Arc<WorkspaceReadService>,
        admission: Arc<ReviewAdmission>,
        limits: ReviewLimits,
    ) -> Self {
        Self {
            workspaces,
            runtime,
            files,
            admission,
            limits,
        }
    }

    /// Returns one bounded page with status derived from current anchors.
    pub async fn list(
        &self,
        task_id: &TaskId,
        cursor: Option<&PageCursor>,
        limit: u16,
    ) -> Result<ReviewSubmissionPage, ApiError> {
        if limit == 0 || limit > self.limits.submissions_page {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "review submissions limit is invalid",
                TRACE_ID,
            ));
        }
        let context = self.runtime.task_context(task_id).await?;
        let mut page = self
            .runtime
            .list_submissions(task_id, cursor, limit)
            .await?;
        if page.task_id != *task_id {
            return Err(ApiError::new(
                ApiErrorCode::Internal,
                "review submission projection is inconsistent",
                TRACE_ID,
            ));
        }
        let mut file_revisions = HashMap::new();
        let capability = self.workspaces.capability(&context.workspace_id)?;
        let git = GitReviewService::with_admission(
            capability,
            self.limits.clone(),
            self.admission.clone(),
        );
        let mut cursor = None;
        let mut git_revisions = HashMap::new();
        loop {
            let snapshot = git.snapshot(cursor.as_ref(), self.page_limit()).await?;
            for entry in snapshot.entries {
                git_revisions.insert(entry.path, entry.revision);
            }
            cursor = snapshot.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        for submission in &mut page.items {
            for note in &mut submission.notes {
                let current = match note.side {
                    AnnotationSide::File => {
                        if !file_revisions.contains_key(&note.path) {
                            file_revisions.insert(
                                note.path.clone(),
                                self.files
                                    .current_revision(&context.workspace_id, &note.path)
                                    .await,
                            );
                        }
                        match file_revisions
                            .get(&note.path)
                            .expect("inserted file revision")
                        {
                            Ok(revision) => revision == &note.revision,
                            Err(error)
                                if matches!(
                                    error.code(),
                                    ApiErrorCode::PayloadTooLarge
                                        | ApiErrorCode::ServerBusy
                                        | ApiErrorCode::WorkspaceUnavailable
                                ) =>
                            {
                                return Err(error.clone())
                            }
                            Err(_) => false,
                        }
                    }
                    AnnotationSide::Old | AnnotationSide::New => git_revisions
                        .get(&note.path)
                        .is_some_and(|revision| revision == &note.revision),
                };
                note.status = if current {
                    AnnotationStatus::Current
                } else {
                    AnnotationStatus::Outdated
                };
            }
        }
        Ok(page)
    }

    fn page_limit(&self) -> u16 {
        self.limits.diff_lines.clamp(1, u16::MAX as u32) as u16
    }
}
