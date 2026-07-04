use std::io;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::metrics::Metrics;

enum Direction {
    ClientToOrigin,
    OriginToClient,
}

pub(crate) async fn bidirectional<L, R>(
    left: L,
    right: R,
    buffer_size: usize,
    metrics: Arc<Metrics>,
) -> Result<(), String>
where
    L: AsyncRead + AsyncWrite + Unpin,
    R: AsyncRead + AsyncWrite + Unpin,
{
    let (mut left_read, mut left_write) = tokio::io::split(left);
    let (mut right_read, mut right_write) = tokio::io::split(right);

    tokio::try_join!(
        copy_direction(
            &mut left_read,
            &mut right_write,
            buffer_size,
            &metrics,
            Direction::ClientToOrigin,
        ),
        copy_direction(
            &mut right_read,
            &mut left_write,
            buffer_size,
            &metrics,
            Direction::OriginToClient,
        )
    )?;

    Ok(())
}

async fn copy_direction<R, W>(
    reader: &mut R,
    writer: &mut W,
    buffer_size: usize,
    metrics: &Metrics,
    direction: Direction,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0; buffer_size];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(read) => read,
            Err(err) if is_peer_closed(&err) => {
                shutdown_writer(writer).await?;
                return Ok(());
            }
            Err(err) => return Err(format!("failed to read relay stream: {err}")),
        };
        if read == 0 {
            shutdown_writer(writer).await?;
            return Ok(());
        }

        if let Err(err) = writer.write_all(&buffer[..read]).await {
            if is_peer_closed(&err) {
                return Ok(());
            }
            return Err(format!("failed to write relay stream: {err}"));
        }
        let bytes =
            u64::try_from(read).map_err(|err| format!("failed to count relayed bytes: {err}"))?;
        match direction {
            Direction::ClientToOrigin => metrics.add_client_to_origin_bytes(bytes),
            Direction::OriginToClient => metrics.add_origin_to_client_bytes(bytes),
        }
    }
}

async fn shutdown_writer<W>(writer: &mut W) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    match writer.shutdown().await {
        Ok(()) => Ok(()),
        Err(err) if is_peer_closed(&err) => Ok(()),
        Err(err) => Err(format!("failed to shutdown relay stream: {err}")),
    }
}

fn is_peer_closed(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe | io::ErrorKind::NotConnected
    )
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use tokio::io::{duplex, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

    use crate::metrics::Metrics;

    struct ResetRead;

    impl AsyncRead for ResetRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::from(io::ErrorKind::ConnectionReset)))
        }
    }

    struct BrokenPipeWrite;

    impl AsyncWrite for BrokenPipeWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn relays_bytes_in_both_directions_and_counts_them() {
        let (mut client, left) = duplex(4096);
        let (right, mut origin) = duplex(4096);
        let metrics = Arc::new(Metrics::default());
        let relay_metrics = Arc::clone(&metrics);

        let relay_task =
            tokio::spawn(async move { super::bidirectional(left, right, 8, relay_metrics).await });

        client.write_all(b"client-to-origin").await.unwrap();
        origin.write_all(b"origin-to-client").await.unwrap();

        let mut origin_received = vec![0; b"client-to-origin".len()];
        origin.read_exact(&mut origin_received).await.unwrap();
        let mut client_received = vec![0; b"origin-to-client".len()];
        client.read_exact(&mut client_received).await.unwrap();

        assert_eq!(origin_received, b"client-to-origin");
        assert_eq!(client_received, b"origin-to-client");

        client.shutdown().await.unwrap();
        origin.shutdown().await.unwrap();
        relay_task.await.unwrap().unwrap();

        let json = metrics.json_snapshot();
        assert!(json.contains("\"bytes_client_to_origin\":16"));
        assert!(json.contains("\"bytes_origin_to_client\":16"));
    }

    #[tokio::test]
    async fn treats_peer_reset_read_as_graceful_close() {
        let mut reader = ResetRead;
        let mut writer = tokio::io::sink();
        let metrics = Metrics::default();

        let result = super::copy_direction(
            &mut reader,
            &mut writer,
            8,
            &metrics,
            super::Direction::ClientToOrigin,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn treats_broken_pipe_write_as_graceful_close() {
        let mut reader = tokio::io::repeat(0).take(1);
        let mut writer = BrokenPipeWrite;
        let metrics = Metrics::default();

        let result = super::copy_direction(
            &mut reader,
            &mut writer,
            8,
            &metrics,
            super::Direction::ClientToOrigin,
        )
        .await;

        assert!(result.is_ok());
    }
}
