//! Plumbing shared by the Cyanotype HTTP services (`apps/auth`, `apps/assets`).
pub mod auth;
pub mod error;
pub mod http;

pub use auth::{AuthUser, SessionUser};
pub use error::{ApiError, ApiResult};
