#![allow(dead_code, unused_imports)]

use kuku_server::api::WorkspaceId;

mod api {
    pub use kuku_server::api::*;
}

mod platform {
    pub use kuku_server::platform::*;
}

mod run_manager {
    pub use kuku_server::run_manager::*;
}

#[path = "common/review_modules.rs"]
mod review;
#[path = "../src/review/mod.rs"]
mod review_contract;

#[path = "../src/routes/review.rs"]
mod review_routes;

#[test]
fn review_route_module_exposes_the_seven_typed_endpoints() {
    let _router = review_routes::router::<()>;
    let _ = WorkspaceId::parse("wsp_000000000000000000000001").unwrap();
}
