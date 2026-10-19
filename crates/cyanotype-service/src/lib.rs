//! Plumbing shared by the Cyanotype HTTP services (`apps/auth`, `apps/assets`).
pub mod audit;
pub mod auth;
pub mod error;
pub mod http;
pub mod ratelimit;

pub use auth::{AdminUser, AnyCaller, AuthUser, EnrolUser, ImportUser, Scope, SessionUser, Unlock};
pub use error::{ApiError, ApiResult};
