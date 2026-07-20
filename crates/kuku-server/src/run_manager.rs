pub mod domain;
pub mod driver;
pub mod idempotency;
pub mod projection;
pub mod repository;
mod repository_support;
pub mod store;
mod submission;
pub mod subscription;
pub mod supervisor;

pub use domain::{DomainError, TaskAggregate};
pub use repository::TaskRepository;
pub use store::{CreateTaskCommand, ResolveInteractionCommand, StopRunCommand, TaskCommandService};
pub use submission::{
    ReviewSubmissionValidator, RunQueueAdmission, RunQueueReservation, SkillSelectionValidator,
    SubmitReviewCommand, SubmitRunCommand, ValidatedSkillSelection,
};
pub use supervisor::{RunSupervisor, TaskRuntime};

#[cfg(test)]
mod command_tests;
#[cfg(test)]
mod domain_tests;
#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod subscription_tests;
#[cfg(test)]
mod supervisor_tests;
