use std::sync::Arc;

use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::server_config::ServerConfig;
use crate::async_graphql_hyper::{GraphQLBatchRequest, GraphQLRequest};
use crate::http::handle_request;

pub async fn start_http_1(
    sc: Arc<ServerConfig>,
    server_up_sender: Option<oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let addr = sc.addr();
    let listener = TcpListener::bind(addr).await?;

    super::log_launch_and_open_browser(sc.as_ref());

    if let Some(sender) = server_up_sender {
        sender
            .send(())
            .or(Err(anyhow::anyhow!("Failed to send message")))?;
    }

    if sc.blueprint.server.enable_batch_requests {
        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let sc = sc.clone();
            let server = hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    io,
                    service_fn(move |req: Request<Incoming>| {
                        let state = sc.clone();
                        async move {
                            let (part, body) = req.into_parts();
                            let body = body.collect().await?.to_bytes();
                            let req = Request::from_parts(part, Full::new(body));
                            handle_request::<GraphQLBatchRequest>(req, state.app_ctx.clone()).await
                        }
                    }),
                )
                .await;
            if let Err(e) = server {
                log::error!("An error occurred while handling a request: {e}");
            }
        }
    } else {
        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let sc = sc.clone();
            tokio::spawn(async move {
                let server = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req: Request<Incoming>| {
                            let state = sc.clone();
                            async move {
                                let (part, body) = req.into_parts();
                                let body = body.collect().await?.to_bytes();
                                let req = Request::from_parts(part, Full::new(body));
                                handle_request::<GraphQLRequest>(req, state.app_ctx.clone()).await
                            }
                        }),
                    )
                    .await;
                if let Err(e) = server {
                    log::error!("An error occurred while handling a request: {e}");
                }
            });
        }
    }
}
