mod tokiort;

pub use self::tokiort::{TokioExecutor, TokioIo, TokioTimer};

use std::convert::Infallible;
use std::error::Error;
use std::marker::Unpin;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::FutureExt;
use http_body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;

pin_project! {
    #[project = EitherProj]
    pub enum Either<L, R> {
        Left {
            #[pin]
            value: L,
        },
        Right {
            #[pin]
            value: R,
        },
    }
}

impl<L, R> Either<L, R> {
    pub fn left(value: L) -> Self {
        Either::Left { value }
    }

    pub fn right(value: R) -> Self {
        Either::Right { value }
    }
}

impl<L: Body, R: Body<Data = L::Data, Error = Infallible>> Body for Either<L, R> {
    type Data = L::Data;
    type Error = L::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.project() {
            EitherProj::Left { value } => value.poll_frame(cx),
            EitherProj::Right { value } => value.poll_frame(cx).map_err(|e| match e {}),
        }
    }

    fn is_end_stream(&self) -> bool {
        match *self {
            Either::Left { ref value } => value.is_end_stream(),
            Either::Right { ref value } => value.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match *self {
            Either::Left { ref value } => value.size_hint(),
            Either::Right { ref value } => value.size_hint(),
        }
    }
}

pub async fn http2_handshake<T, B>(
    io: T,
) -> hyper::Result<hyper::client::conn::http2::SendRequest<B>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    B: Body + Send + Unpin + 'static,
    B::Data: Send,
    B::Error: Error + Send + Sync,
{
    let (ret, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor)
        .timer(TokioTimer)
        .handshake(TokioIo::new(io))
        .await?;

    tokio::spawn(conn.map(|result| {
        if let Err(error) = result {
            tracing::error!(%error, "Error in HTTP connection");
        }
    }));

    Ok(ret)
}
