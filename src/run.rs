use std::convert::Infallible;
use std::path::PathBuf;

use atom::Feed;
use bytes::Bytes;
use chrono::Utc;
use http::header::CONTENT_TYPE;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use tokio::net::{UnixListener, UnixStream};
use tracing::instrument;

use crate::util::{http2_handshake, Either, TokioIo, TokioTimer};

#[instrument(skip_all, fields(server_addr = ?client_sock))]
pub async fn main(client_sock: PathBuf, incoming: UnixListener) -> anyhow::Result<()> {
    tracing::debug!("Connecting HTTP client");
    let client_stream = UnixStream::connect(&client_sock).await?;
    let mut send_request =
        http2_handshake::<UnixStream, Either<Incoming, Full<Bytes>>>(client_stream).await?;

    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer);

    loop {
        let stream = match incoming.accept().await {
            Ok((stream, _addr)) => stream,
            Err(error) => {
                tracing::error!(%error, "Error accepting incoming connection");
                continue;
            }
        };

        if let Err(error) = send_request.ready().await {
            tracing::info!(%error, "HTTP client connection has closed");
            tracing::debug!("Reconnecting HTTP client");
            let client_stream = UnixStream::connect(&client_sock).await?;
            send_request = http2_handshake(client_stream).await?;
        };
        let send_request = send_request.clone();

        let service = service_fn(move |req| {
            let mut send_request = send_request.clone();
            async move {
                let fut = handle(req, &mut send_request);
                match fut.await {
                    Ok(res) => Ok::<_, Infallible>(res),
                    Err(error) => {
                        tracing::error!(%error, "Error serving HTTP request");
                        Ok(Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Either::right(Empty::new()))
                            .unwrap())
                    }
                }
            }
        });
        let fut = http.serve_connection(TokioIo::new(stream), service);
        tokio::spawn(async move {
            if let Err(error) = fut.await {
                tracing::error!(%error, "Error serving HTTP request");
            }
        });
    }
}

#[instrument(skip_all, fields(method = %req.method(), uri = %req.uri()))]
async fn handle(
    req: Request<Incoming>,
    send_request: &mut hyper::client::conn::http2::SendRequest<Either<Incoming, Full<Bytes>>>,
) -> hyper::Result<Response<Either<Incoming, Empty<Bytes>>>> {
    if !req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map_or(false, |v| v.starts_with("application/atom+xml"))
    {
        tracing::info!("Not Atom feed; passing through");
        let res = send_request.send_request(req.map(Either::left)).await?;
        return Ok(res.map(Either::left));
    }

    let (parts, body) = req.into_parts();
    let body = body.collect().await?.to_bytes();

    let feed = match Feed::read_from(&body[..]) {
        Ok(feed) => feed,
        Err(error) => {
            tracing::warn!(%error, "Error parsing Atom feed");
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Either::right(Empty::new()))
                .unwrap());
        }
    };

    let now = Utc::now();
    let threshold = now - chrono::Duration::days(1);
    if feed
        .entries
        .iter()
        .all(|e| e.published.map_or(false, |t| t < threshold))
    {
        tracing::info!("Blocking too old entry");
        return Ok(Response::new(Either::right(Empty::new())));
    }

    tracing::info!("New entry; passing through");
    let req = Request::from_parts(parts, Either::right(Full::new(body)));
    let res = send_request.send_request(req).await?;
    Ok(res.map(Either::left))
}
