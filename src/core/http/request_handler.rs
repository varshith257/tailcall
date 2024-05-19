use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::ops::Deref;

use anyhow::Result;
use async_graphql::ServerError;
use hyper::header::{self, CONTENT_TYPE};
use hyper::http::Method;
use hyper::{Body, HeaderMap, Request, Response, StatusCode};
use opentelemetry::trace::SpanKind;
use opentelemetry_semantic_conventions::trace::{HTTP_REQUEST_METHOD, HTTP_ROUTE};
use prometheus::{Encoder, ProtobufEncoder, TextEncoder, TEXT_FORMAT};
use serde::de::DeserializeOwned;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::request_context::RequestContext;
use super::telemetry::{get_response_status_code, RequestCounter};
use super::{showcase, telemetry, AppContext, TAILCALL_HTTPS_ORIGIN, TAILCALL_HTTP_ORIGIN};
use crate::core::async_graphql_hyper::{GraphQLRequestLike, GraphQLResponse};
use crate::core::blueprint::telemetry::TelemetryExporter;
use crate::core::config::{PrometheusExporter, PrometheusFormat};

pub const API_URL_PREFIX: &str = "/api";

// Middleware for Prometheus metrics
async fn prometheus_metrics_middleware(
    req: Request<Body>,
    app_ctx: Arc<AppContext>,
) -> Result<Option<Response<Body>>> {
    if req.uri().path() == "/metrics" {
        let prometheus_exporter = app_ctx.blueprint.telemetry.export.as_ref().unwrap();
        let metric_families = prometheus::default_registry().gather();
        let mut buffer = vec![];

        match prometheus_exporter.format {
            PrometheusFormat::Text => TextEncoder::new().encode(&metric_families, &mut buffer)?,
            PrometheusFormat::Protobuf => {
                ProtobufEncoder::new().encode(&metric_families, &mut buffer)?
            }
        };

        let content_type = match prometheus_exporter.format {
            PrometheusFormat::Text => TEXT_FORMAT,
            PrometheusFormat::Protobuf => prometheus::PROTOBUF_FORMAT,
        };

        let response = Response::builder()
            .status(200)
            .header(CONTENT_TYPE, content_type)
            .body(Body::from(buffer))?;
        return Ok(Some(response));
    }
    Ok(None)
}

// Middleware for CORS
async fn cors_middleware(
    req: Request<Body>,
    app_ctx: Arc<AppContext>,
) -> Result<Option<Response<Body>>> {
    if let Some(cors) = app_ctx.blueprint.server.cors.as_ref() {
        let (parts, body) = req.into_parts();
        let origin = parts.headers.get(&header::ORIGIN);

        let mut headers = HeaderMap::new();
        headers.extend(cors.allow_origin_to_header(origin));
        headers.extend(cors.allow_credentials_to_header());
        headers.extend(cors.allow_private_network_to_header(&parts));
        headers.extend(cors.vary_to_header());

        if parts.method == Method::OPTIONS {
            headers.extend(cors.allow_methods_to_header());
            headers.extend(cors.allow_headers_to_header());
            headers.extend(cors.max_age_to_header());

            // Ensure ACCESS_CONTROL_ALLOW_HEADERS is included
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("*"));
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("https://tailcall.run"));
            headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, OPTIONS"));

            let mut response = Response::new(Body::default());
            std::mem::swap(response.headers_mut(), &mut headers);

            return Ok(Some(response));
        } else {
            headers.extend(cors.expose_headers_to_header());

            let req = Request::from_parts(parts, body);
            let mut response = handle_request_inner::<T>(req, app_ctx, request_counter).await?;

            let response_headers = response.headers_mut();
            if let Some(vary) = headers.remove(header::VARY) {
                response_headers.append(header::VARY, vary);
            }
            response_headers.extend(headers.drain());

            // Ensure ACCESS_CONTROL_ALLOW_HEADERS is included in the response
            response_headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("*"));
            response_headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("https://tailcall.run"));
            response_headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, OPTIONS"));

            return Ok(Some(response));
        }
    }
    Ok(None)
}

// Middleware for GraphQL requests
async fn graphql_middleware<T: DeserializeOwned + GraphQLRequestLike>(
    req: Request<Body>,
    app_ctx: Arc<AppContext>,
    req_counter: &mut RequestCounter,
) -> Result<Option<Response<Body>>> {
    if req.uri().path() == "/graphql" {
        req_counter.set_http_route("/graphql");
        let req_ctx = Arc::new(create_request_context(&req, app_ctx));
        let bytes = hyper::body::to_bytes(req.into_body()).await?;
        let graphql_request = serde_json::from_slice::<T>(&bytes);
        match graphql_request {
            Ok(request) => {
                let mut response = request.data(req_ctx.clone()).execute(&app_ctx.schema).await;

                response = update_cache_control_header(response, app_ctx, req_ctx.clone());
                let mut resp = response.into_response()?;
                update_response_headers(&mut resp, &req_ctx, app_ctx);
                return Ok(Some(resp));
            }
            Err(err) => {
                tracing::error!(
                    "Failed to parse request: {}",
                    String::from_utf8(bytes.to_vec()).unwrap()
                );

                let mut response = async_graphql::Response::default();
                let server_error =
                    ServerError::new(format!("Unexpected GraphQL Request: {}", err), None);
                response.errors = vec![server_error];

                return Ok(Some(GraphQLResponse::from(response).into_response()?));
            }
        }
    }
    Ok(None)
}

// Middleware for REST API requests
async fn rest_api_middleware(
    req: Request<Body>,
    app_ctx: Arc<AppContext>,
    req_counter: &mut RequestCounter,
) -> Result<Option<Response<Body>>> {
    if req.uri().path().starts_with(API_URL_PREFIX) {
        *request.uri_mut() = request.uri().path().replace(API_URL_PREFIX, "").parse()?;
        let req_ctx = Arc::new(create_request_context(&request, app_ctx.as_ref()));
        if let Some(p_request) = app_ctx.endpoints.matches(&request) {
            let http_route = format!("{API_URL_PREFIX}{}", p_request.path.as_str());
            req_counter.set_http_route(&http_route);
            let span = tracing::info_span!(
                "REST",
                otel.name = format!("REST {} {}", request.method(), p_request.path.as_str()),
                otel.kind = ?SpanKind::Server,
                { HTTP_REQUEST_METHOD } = %request.method(),
                { HTTP_ROUTE } = http_route
            );
            return async {
                let graphql_request = p_request.into_request(request).await?;
                let mut response = graphql_request
                    .data(req_ctx.clone())
                    .execute(&app_ctx.schema)
                    .await;
                response = update_cache_control_header(response, app_ctx.as_ref(), req_ctx.clone());
                let mut resp = response.into_rest_response()?;
                update_response_headers(&mut resp, &req_ctx, &app_ctx);
                Ok(Some(resp))
            }
            .instrument(span)
            .await;
        }

        return Ok(Some(not_found()?));
    }
    Ok(None)
}

// Middleware for updating response headers
fn response_headers_middleware(
    resp: &mut hyper::Response<hyper::Body>,
    req_ctx: &RequestContext,
    app_ctx: &AppContext,
) {
    set_headers(resp.headers_mut(), &app_ctx.blueprint.server.response_headers, req_ctx.cookie_headers.as_ref());
    req_ctx.extend_x_headers(resp.headers_mut());
}

// Main request handler integrating middleware
#[tracing::instrument(
    skip_all,
    err,
    fields(
        otel.name = "request",
        otel.kind = ?SpanKind::Server,
        url.path = %req.uri().path(),
        http.request.method = %req.method()
    )
)]
pub async fn handle_request<T: DeserializeOwned + GraphQLRequestLike>(
    req: Request<Body>,
    app_ctx: Arc<AppContext>,
) -> Result<Response<Body>> {
    telemetry::propagate_context(&req);
    let mut req_counter = RequestCounter::new(&app_ctx.blueprint.telemetry, &req);

    if let Some(response) = prometheus_metrics_middleware(req, app_ctx.clone()).await? {
        return Ok(response);
    }

    if let Some(response) = cors_middleware(req, app_ctx.clone()).await? {
        return Ok(response);
    }

    if let Some(response) = graphql_middleware(req, app_ctx.clone(), &mut req_counter).await? {
        return Ok(response);
    }

    if let Some(response) = rest_api_middleware(req, app_ctx.clone(), &mut req_counter).await? {
        return Ok(response);
    }

    let response = handle_request_inner::<T>(req, app_ctx, &mut req_counter).await?;
    req_counter.update(&response);
    if let Ok(response) = &response {
        let status = get_response_status_code(response);
        tracing::Span::current().set_attribute(status.key, status.value);
    };

    Ok(response)
}

#[cfg(test)]
mod test {
    #[test]
    fn test_create_allowed_headers() {
        use std::collections::BTreeSet;

        use hyper::header::{HeaderMap, HeaderValue};

        use super::create_allowed_headers;

        let mut headers = HeaderMap::new();
        headers.insert("X-foo", HeaderValue::from_static("bar"));
        headers.insert("x-bar", HeaderValue::from_static("foo"));
        headers.insert("x-baz", HeaderValue::from_static("baz"));

        let allowed = BTreeSet::from_iter(vec!["x-foo".to_string(), "X-bar".to_string()]);

        let new_headers = create_allowed_headers(&headers, &allowed);
        assert_eq!(new_headers.len(), 2);
        assert_eq!(new_headers.get("x-foo").unwrap(), "bar");
        assert_eq!(new_headers.get("x-bar").unwrap(), "foo");
    }
}
