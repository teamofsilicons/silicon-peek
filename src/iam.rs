//! The IAM seam.
//!
//! [`IamPlane`] is everything peek-server asks IAM, for one data plane
//! (production, or one testing environment). [`IamConnector`] hands out the
//! production plane and resolves testing planes from a peek test app secret.
//! The production implementation wraps `silicon-iam-client` 5.0.0; tests run
//! the same implementation against a local mock IAM base URL.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::http::StatusCode;
use serde_json::{Value, json};
use silicon_iam_client::{Client, Credential, EnvironmentKey, Error as IamError, Mutation, models};
use silicon_peek_client::{ErrorCode, identity::TestingSecret};
use uuid::Uuid;

use crate::{config::IamConfig, error::ApiError};

/// Result alias for SDK calls.
pub(crate) type IamResult<T> = Result<T, IamError>;

/// How a token is revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// `oat_…`.
    Access,
    /// `ort_…`: revokes the whole family.
    Refresh,
}

/// IAM, as seen from one data plane.
#[async_trait]
pub(crate) trait IamPlane: Send + Sync {
    /// The testing environment, or `None` for production.
    fn environment_id(&self) -> Option<Uuid>;
    /// Exchanges an SLT (`POST /api/v1/app-auth/tokens`).
    async fn login(
        &self,
        slt: &str,
        org: Option<&str>,
        mutation: &Mutation,
    ) -> IamResult<models::OAuthTokenResponse>;
    /// Rotates a refresh token.
    async fn refresh(
        &self,
        refresh_token: &str,
        mutation: &Mutation,
    ) -> IamResult<models::OAuthTokenResponse>;
    /// Live introspection of an access token.
    async fn introspect(
        &self,
        access_token: &str,
        org: Option<&str>,
    ) -> IamResult<models::TokenIntrospection>;
    /// Revokes a token (unknown tokens succeed).
    async fn revoke(&self, token: &str, kind: TokenKind, mutation: &Mutation) -> IamResult<()>;
    /// The scope-projected `GET /api/v1/me` for the token's actor.
    async fn me(&self, access_token: &str) -> IamResult<Value>;
    /// Starts independent, feature-specific Ting consent.
    async fn obo_authorize(
        &self,
        request: &models::OboAuthorizationRequest,
        mutation: &Mutation,
    ) -> IamResult<models::OboConsentDetail>;
    /// Reads the authorization with this app's credentials.
    async fn obo_authorization(&self, id: Uuid) -> IamResult<models::OboConsentDetail>;
    /// Redeems a one-use code, retaining the same mutation after uncertain I/O.
    async fn obo_code(
        &self,
        id: Uuid,
        code: &str,
        mutation: &Mutation,
    ) -> IamResult<models::OboTokenResponse>;
    /// Rotates one independently stored root family.
    async fn obo_refresh(
        &self,
        refresh: &str,
        mutation: &Mutation,
    ) -> IamResult<models::OboTokenResponse>;
    /// Validates an audience's testing credential (from an approved root's
    /// `testing_context`) and returns the environment it names.
    async fn audience_testing_context(
        &self,
        audience: &str,
        app_secret: &str,
        environment_key: &str,
    ) -> IamResult<models::ApplicationTestingContext>;
}

/// Produces [`IamPlane`]s.
#[async_trait]
pub(crate) trait IamConnector: Send + Sync {
    /// The production plane; `None` while `PEEK_IAM_APP_SECRET` is empty.
    fn production(&self) -> Option<Arc<dyn IamPlane>>;
    /// Resolves a testing plane from a peek test app secret, returning IAM's
    /// testing context for it (the caller validates it).
    async fn testing(
        &self,
        secret: &TestingSecret,
    ) -> IamResult<(Arc<dyn IamPlane>, models::ApplicationTestingContext)>;
}

/// The SDK-backed plane.
struct SdkPlane {
    client: Client,
    app_id: String,
    environment_id: Option<Uuid>,
}

#[async_trait]
impl IamPlane for SdkPlane {
    fn environment_id(&self) -> Option<Uuid> {
        self.environment_id
    }

    async fn login(
        &self,
        slt: &str,
        org: Option<&str>,
        mutation: &Mutation,
    ) -> IamResult<models::OAuthTokenResponse> {
        if self.environment_id.is_some()
            && silicon_peek_client::identity::ActorId::looks_like_public_id(slt)
            && let Some(org) = org
        {
            return self
                .client
                .oauth()
                .login_testing_actor(&self.app_id, slt, org, mutation)
                .await;
        }
        self.client.oauth().login(&self.app_id, slt, mutation).await
    }

    async fn refresh(
        &self,
        refresh_token: &str,
        mutation: &Mutation,
    ) -> IamResult<models::OAuthTokenResponse> {
        self.client
            .oauth()
            .refresh(&self.app_id, refresh_token, mutation)
            .await
    }

    async fn introspect(
        &self,
        access_token: &str,
        org: Option<&str>,
    ) -> IamResult<models::TokenIntrospection> {
        self.client
            .oauth()
            .introspect(
                &models::TokenIntrospectionRequest {
                    token: access_token.to_owned(),
                    token_type_hint: Some(
                        models::TokenIntrospectionRequestTokenTypeHint::AccessToken,
                    ),
                },
                org,
            )
            .await
    }

    async fn revoke(&self, token: &str, kind: TokenKind, mutation: &Mutation) -> IamResult<()> {
        let hint = match kind {
            TokenKind::Access => models::OAuthRevocationRequestTokenTypeHint::AccessToken,
            TokenKind::Refresh => models::OAuthRevocationRequestTokenTypeHint::RefreshToken,
        };
        self.client
            .oauth()
            .revoke(
                &models::OAuthRevocationRequest {
                    token: token.to_owned(),
                    token_type_hint: Some(hint),
                },
                mutation,
            )
            .await
    }

    async fn me(&self, access_token: &str) -> IamResult<Value> {
        self.client
            .with_credential(Credential::bearer(access_token))
            .application_reads()
            .me()
            .await
    }

    async fn obo_authorize(
        &self,
        request: &models::OboAuthorizationRequest,
        mutation: &Mutation,
    ) -> IamResult<models::OboConsentDetail> {
        self.client.obo().authorize(request, mutation).await
    }
    async fn obo_authorization(&self, id: Uuid) -> IamResult<models::OboConsentDetail> {
        self.client.obo().authorization(id).await
    }
    async fn obo_code(
        &self,
        id: Uuid,
        code: &str,
        mutation: &Mutation,
    ) -> IamResult<models::OboTokenResponse> {
        self.client.obo().exchange_code(id, code, mutation).await
    }
    async fn obo_refresh(
        &self,
        refresh: &str,
        mutation: &Mutation,
    ) -> IamResult<models::OboTokenResponse> {
        self.client.obo().refresh(refresh, mutation).await
    }

    async fn audience_testing_context(
        &self,
        audience: &str,
        app_secret: &str,
        environment_key: &str,
    ) -> IamResult<models::ApplicationTestingContext> {
        let key = EnvironmentKey::new(environment_key.to_owned())?;
        self.client
            .with_credential(Credential::application(audience, app_secret))
            .with_environment(key)
            .applications()
            .testing_context()
            .await
    }
}

/// The SDK-backed connector.
pub(crate) struct SdkConnector {
    base: Client,
    app_id: String,
    production: Option<Arc<dyn IamPlane>>,
}

impl SdkConnector {
    /// Builds the connector (no network I/O).
    pub(crate) fn new(config: &IamConfig) -> Result<Self, IamError> {
        let base = Client::builder(&config.base_url)?
            .timeout(config.request_timeout)
            .user_agent(concat!("silicon-peek/", env!("CARGO_PKG_VERSION")))
            .telemetry(config.sdk_telemetry)
            .build()?;
        let production = config.app_secret.as_ref().map(|secret| {
            Arc::new(SdkPlane {
                client: base.with_credential(Credential::application(
                    config.app_id.clone(),
                    secret.expose().to_owned(),
                )),
                app_id: config.app_id.clone(),
                environment_id: None,
            }) as Arc<dyn IamPlane>
        });
        Ok(Self {
            base,
            app_id: config.app_id.clone(),
            production,
        })
    }
}

#[async_trait]
impl IamConnector for SdkConnector {
    fn production(&self) -> Option<Arc<dyn IamPlane>> {
        self.production.clone()
    }

    async fn testing(
        &self,
        secret: &TestingSecret,
    ) -> IamResult<(Arc<dyn IamPlane>, models::ApplicationTestingContext)> {
        let exposed = secret.secret().expose();
        let client = self
            .base
            .with_testing_application(&self.app_id, exposed)?
            .with_credential(Credential::application(
                self.app_id.clone(),
                exposed.to_owned(),
            ));
        let context = client.applications().testing_context().await?;
        let plane = SdkPlane {
            client,
            app_id: self.app_id.clone(),
            environment_id: Some(context.environment_id),
        };
        Ok((Arc::new(plane), context))
    }
}

/// The IAM error code, when IAM answered with its envelope.
pub(crate) fn api_code(error: &IamError) -> Option<(&str, u16)> {
    error.api().map(|api| (api.code.as_str(), api.status))
}

/// Maps an IAM failure that no operation-specific rule handled.
///
/// `testing` selects how `invalid_client` reads: in production it is the
/// operator's app secret (`iam_misconfigured`), in a testing plane it is the
/// caller's test secret (`testing_secret_invalid`).
pub(crate) fn upstream(error: &IamError, operation: &str, testing: bool) -> ApiError {
    match error {
        IamError::Api(api) if api.code == "invalid_client" => {
            if testing {
                ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    ErrorCode::TestingSecretInvalid,
                    format!("IAM no longer accepts this testing app secret ({operation} failed with invalid_client)"),
                )
                .with_hint("the secret was rotated or the environment was cleaned; get the current one with `honeycomb --test <env> apps rotate-secret 'peek'`")
            } else {
                ApiError::iam_misconfigured(format!(
                    "IAM rejected peek-server's app credentials while trying to {operation} (invalid_client)"
                ))
            }
        }
        IamError::RateLimited { retry_after, .. } => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IamUnavailable,
            format!("IAM is rate limiting peek-server; could not {operation}"),
        )
        .with_hint("retry after the indicated delay")
        .with_retry_after(retry_after.as_secs().max(1)),
        IamError::Transport(e) => {
            let why = if e.is_timeout() {
                "timed out"
            } else if e.is_connect() {
                "could not connect"
            } else {
                "the connection failed"
            };
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::IamUnavailable,
                format!("peek-server could not reach IAM to {operation}: {why}"),
            )
            .with_hint("retry; IAM may be briefly unavailable")
            .with_retry_after(2)
        }
        IamError::Api(api) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IamUnavailable,
            format!(
                "IAM refused to {operation}: HTTP {} {}",
                api.status, api.code
            ),
        )
        .with_hint(if api.status >= 500 {
            "retry; IAM may be briefly unavailable"
        } else {
            "this refusal is unexpected; report it with `peek report` and quote the request ID"
        })
        .with_details(json!({"iam_code": api.code, "iam_status": api.status, "iam_request_id": api.request_id}))
        .with_retryable(api.status >= 500),
        IamError::UnstructuredResponse { status, request_id } => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IamUnavailable,
            format!("IAM answered HTTP {status} without an error envelope while peek-server tried to {operation}"),
        )
        .with_hint("retry; a proxy in front of IAM answered instead of IAM")
        .with_details(json!({"iam_status": status, "iam_request_id": request_id}))
        .with_retryable(true),
        IamError::Decode(_)
        | IamError::ResponseTooLarge { .. }
        | IamError::ApiVersionUnsupported { .. } => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IamUnavailable,
            format!("IAM answered with a response peek-server cannot read while trying to {operation}"),
        )
        .with_hint("this usually means IAM and peek-server disagree on the API version; operators should check both")
        .with_retryable(true),
        IamError::Invalid(why) => {
            tracing::error!(operation, why, "IAM client refused a request locally");
            ApiError::internal(format!("peek-server built an invalid IAM request to {operation}"))
        }
        _ => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IamUnavailable,
            format!("IAM failed in an unexpected way while peek-server tried to {operation}"),
        )
        .with_retryable(true),
    }
}

/// The request timeout budget for best-effort IAM reads.
pub(crate) const PROFILE_TIMEOUT: Duration = Duration::from_secs(3);

/// Validates an access or refresh token's shape before it is sent anywhere.
pub(crate) fn token_shape_ok(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 8192
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16, code: &str) -> IamError {
        IamError::Api(Box::new(silicon_iam_client::ApiError {
            status,
            code: code.into(),
            message: "m".into(),
            details: None,
            request_id: None,
        }))
    }

    #[test]
    fn invalid_client_is_an_operator_problem_only_in_production() {
        let e = upstream(&api(401, "invalid_client"), "log in", false);
        assert_eq!(*e.code(), ErrorCode::IamMisconfigured);
        assert_eq!(e.status(), StatusCode::SERVICE_UNAVAILABLE);
        let e = upstream(&api(401, "invalid_client"), "log in", true);
        assert_eq!(*e.code(), ErrorCode::TestingSecretInvalid);
        assert_eq!(e.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn outages_are_retryable() {
        let e = upstream(&api(503, "unavailable"), "introspect", false);
        assert_eq!(*e.code(), ErrorCode::IamUnavailable);
        let e = upstream(
            &IamError::UnstructuredResponse {
                status: 502,
                request_id: None,
            },
            "introspect",
            false,
        );
        assert_eq!(e.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn token_shapes() {
        assert!(token_shape_ok("oat_abc-DEF_1", "oat_"));
        assert!(!token_shape_ok("oat_", "oat_"));
        assert!(!token_shape_ok("ort_abc", "oat_"));
        assert!(!token_shape_ok("oat_a b", "oat_"));
    }
}
