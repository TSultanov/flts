//! Applies `Action`s around the sim's real handlers.

use crate::{
    rules::{Action, CorruptMode},
    server::{RequestRecord, SimState},
};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BODY: usize = 8 * 1024 * 1024;
/// Non-standard; the aborted connection makes it moot anyway.
const STALL_ABORTED: u16 = 599;
const GARBAGE: &[u8; 64] =
    b"\x00\xffGARBAGE\x01\x02\x03not-json-at-all\x7f\xfe\xfd~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~";

pub fn wrap(inner: Router, state: Arc<SimState>) -> Router {
    inner.layer(middleware::from_fn_with_state(state, fault_layer))
}

pub async fn fault_layer(State(state): State<Arc<SimState>>, req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let method = parts.method.to_string();
    let path = parts.uri.path().to_string();
    // Rules match on the bare path; the log keeps the query so tests can assert on params.
    let logged_path = parts
        .uri
        .path_and_query()
        .map_or_else(|| path.clone(), ToString::to_string);
    let bytes = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("body read failed: {e}")).into_response();
        }
    };

    state.log.lock().unwrap().push(RequestRecord {
        method: method.clone(),
        path: logged_path,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        ts_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    });
    let action = state.rules.lock().unwrap().decide(&method, &path, &bytes);

    let req = Request::from_parts(parts, Body::from(bytes));
    match action {
        Action::Passthrough => next.run(req).await,
        Action::Status { code, body } => status_response(code, body),
        Action::Delay { ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            next.run(req).await
        }
        Action::Stall => {
            state.stall_abort.notified().await;
            StatusCode::from_u16(STALL_ABORTED)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
        Action::Drop { after_bytes } => {
            let (parts, body) = split(next.run(req).await).await;
            let n = after_bytes.unwrap_or(0).min(body.len());
            // Headers keep the full Content-Length, so the error aborts mid-message.
            Response::from_parts(parts, errored_body(body.slice(..n)))
        }
        Action::Truncate { fraction } => {
            let (parts, body) = split(next.run(req).await).await;
            let n = ((body.len() as f32 * fraction.clamp(0.0, 1.0)) as usize).min(body.len());
            // Streamed so hyper keeps the full body's Content-Length: the client
            // sees a message that ends short of what the headers promised.
            Response::from_parts(parts, truncated_body(body.slice(..n), body.len()))
        }
        Action::Corrupt { mode } => {
            let (mut parts, body) = split(next.run(req).await).await;
            parts.headers.remove(header::CONTENT_LENGTH);
            match mode {
                CorruptMode::MalformedJson => {
                    let mut v = Vec::with_capacity(body.len() + 1);
                    v.push(b'{');
                    v.extend_from_slice(&body);
                    if let Some(i) = v.iter().rposition(|b| *b == b'}') {
                        v.remove(i);
                    }
                    Response::from_parts(parts, Body::from(v))
                }
                CorruptMode::WrongContentType => {
                    parts.headers.insert(
                        header::CONTENT_TYPE,
                        header::HeaderValue::from_static("text/html"),
                    );
                    Response::from_parts(parts, Body::from(body))
                }
                CorruptMode::Garbage => Response::from_parts(parts, Body::from(GARBAGE.as_slice())),
            }
        }
    }
}

fn status_response(code: u16, body: Option<serde_json::Value>) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = body.unwrap_or_else(|| serde_json::json!({}));
    (status, axum::Json(body)).into_response()
}

async fn split(resp: Response) -> (axum::http::response::Parts, Bytes) {
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, MAX_BODY).await.unwrap_or_default();
    (parts, bytes)
}

/// Prefix, then a stream error hyper turns into an abrupt close.
fn errored_body(prefix: Bytes) -> Body {
    let items: Vec<Result<Bytes, io::Error>> =
        vec![Ok(prefix), Err(io::Error::other("sim: connection dropped"))];
    Body::from_stream(futures_util::stream::iter(items))
}

/// Yields `prefix` but promises `promised` bytes, so the message ends short of
/// its Content-Length. An unknown size hint would make hyper switch to chunked.
struct ShortBody {
    prefix: Option<Bytes>,
    promised: u64,
}

impl http_body::Body for ShortBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, io::Error>>> {
        Poll::Ready(self.prefix.take().map(|b| Ok(http_body::Frame::data(b))))
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::with_exact(self.promised)
    }
}

fn truncated_body(prefix: Bytes, promised: usize) -> Body {
    Body::new(ShortBody {
        prefix: Some(prefix),
        promised: promised as u64,
    })
}
