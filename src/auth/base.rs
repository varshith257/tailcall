use std::sync::Arc;

use thiserror::Error;

use super::basic::BasicProvider;
use super::jwt::JwtProvider;
use crate::blueprint;
use crate::http::RequestContext;
use crate::io::HttpIO;

#[derive(Debug, Error, Clone, PartialEq, PartialOrd)]
pub enum AuthError {
  #[error("Haven't found auth parameters")]
  Missing,
  #[error("Couldn't validate auth request")]
  // in case we haven't managed to actually validate the request
  // and have failed somewhere else, usually while executing request
  ValidationCheckFailed,
  #[error("Auth validation failed")]
  Invalid,
}

pub(crate) trait AuthProviderTrait {
  async fn validate(&self, req_ctx: &RequestContext) -> Result<(), AuthError>;
}

pub enum AuthProvider {
  Basic(BasicProvider),
  Jwt(JwtProvider),
}

impl AuthProvider {
  pub fn from_config(config: blueprint::AuthProvider, client: Arc<dyn HttpIO>) -> Self {
    match config {
      blueprint::AuthProvider::Basic(options) => AuthProvider::Basic(BasicProvider::new(options)),
      blueprint::AuthProvider::Jwt(options) => AuthProvider::Jwt(JwtProvider::new(options, client)),
    }
  }
}

impl AuthProviderTrait for AuthProvider {
  async fn validate(&self, req_ctx: &RequestContext) -> Result<(), AuthError> {
    match self {
      AuthProvider::Basic(basic) => basic.validate(req_ctx).await,
      AuthProvider::Jwt(jwt) => jwt.validate(req_ctx).await,
    }
  }
}