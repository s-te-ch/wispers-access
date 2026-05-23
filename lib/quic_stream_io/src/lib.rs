//! Adapter that lets hyper drive I/O on a `wispers_connect::QuicStream`.
//!
//! The public surface is just [`wrap`], which returns an opaque value that
//! implements hyper's [`Read`](hyper::rt::Read) + [`Write`](hyper::rt::Write)
//! traits. Internally, it's a state-machine adapter from `QuicStream`'s
//! async-fn API to tokio's poll-based `AsyncRead`/`AsyncWrite`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const READ_CHUNK: usize = 16 * 1024;

/// Wrap a `QuicStream` into a value hyper can use as a connection I/O type.
pub fn wrap(
    stream: wispers_connect::QuicStream,
) -> impl hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static {
    TokioIo::new(QuicStreamIo::new(stream))
}

struct QuicStreamIo {
    stream: Arc<wispers_connect::QuicStream>,
    read_state: ReadState,
    write_state: WriteState,
    shutdown_state: ShutdownState,
}

type IoFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

enum ReadState {
    Idle,
    Reading(IoFuture<(Vec<u8>, usize)>),
}

enum WriteState {
    Idle,
    Writing(IoFuture<usize>),
}

enum ShutdownState {
    NotStarted,
    Finishing(IoFuture<()>),
    Done,
}

impl QuicStreamIo {
    fn new(stream: wispers_connect::QuicStream) -> Self {
        Self {
            stream: Arc::new(stream),
            read_state: ReadState::Idle,
            write_state: WriteState::Idle,
            shutdown_state: ShutdownState::NotStarted,
        }
    }
}

fn map_err(e: wispers_connect::P2pError) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

impl AsyncRead for QuicStreamIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.read_state {
                ReadState::Idle => {
                    let stream = Arc::clone(&this.stream);
                    let want = buf.remaining().min(READ_CHUNK);
                    let fut: IoFuture<(Vec<u8>, usize)> = Box::pin(async move {
                        let mut tmp = vec![0u8; want];
                        let n = stream.read(&mut tmp).await.map_err(map_err)?;
                        Ok((tmp, n))
                    });
                    this.read_state = ReadState::Reading(fut);
                }
                ReadState::Reading(fut) => match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok((tmp, n))) => {
                        this.read_state = ReadState::Idle;
                        buf.put_slice(&tmp[..n]);
                        return Poll::Ready(Ok(()));
                    }
                    Poll::Ready(Err(e)) => {
                        this.read_state = ReadState::Idle;
                        return Poll::Ready(Err(e));
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}

impl AsyncWrite for QuicStreamIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            match &mut this.write_state {
                WriteState::Idle => {
                    let stream = Arc::clone(&this.stream);
                    let data = buf.to_vec();
                    let fut: IoFuture<usize> =
                        Box::pin(async move { stream.write(&data).await.map_err(map_err) });
                    this.write_state = WriteState::Writing(fut);
                }
                WriteState::Writing(fut) => match fut.as_mut().poll(cx) {
                    Poll::Ready(res) => {
                        this.write_state = WriteState::Idle;
                        return Poll::Ready(res);
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // QuicStream writes complete via the future above; no separate flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.shutdown_state {
                ShutdownState::NotStarted => {
                    let stream = Arc::clone(&this.stream);
                    let fut: IoFuture<()> =
                        Box::pin(async move { stream.finish().await.map_err(map_err) });
                    this.shutdown_state = ShutdownState::Finishing(fut);
                }
                ShutdownState::Finishing(fut) => match fut.as_mut().poll(cx) {
                    Poll::Ready(res) => {
                        this.shutdown_state = ShutdownState::Done;
                        return Poll::Ready(res);
                    }
                    Poll::Pending => return Poll::Pending,
                },
                ShutdownState::Done => return Poll::Ready(Ok(())),
            }
        }
    }
}
