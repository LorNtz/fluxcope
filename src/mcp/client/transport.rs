use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use bytes::BytesMut;
use futures::StreamExt;
use rmcp::{
    RoleClient,
    model::ClientRequest,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{
        Transport,
        async_rw::{JsonRpcMessageCodec, JsonRpcMessageCodecError},
    },
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::{Encoder, FramedRead};

use super::ClientFailure;

#[cfg(unix)]
pub(super) const REQUEST_MAX_BYTES: usize = crate::mcp::stdio::MCP_STDIO_REQUEST_MAX_BYTES;
#[cfg(unix)]
const RESPONSE_MAX_BYTES: usize = crate::mcp::stdio::MCP_STDIO_RESPONSE_MAX_BYTES;
#[cfg(not(unix))]
pub(super) const REQUEST_MAX_BYTES: usize = 1024 * 1024;
#[cfg(not(unix))]
const RESPONSE_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Default)]
pub(super) struct TransportState {
    dispatched: Arc<AtomicBool>,
    failure: Arc<parking_lot::Mutex<Option<ClientFailure>>>,
}

impl TransportState {
    pub(super) fn dispatched(&self) -> bool {
        self.dispatched.load(Ordering::Acquire)
    }

    pub(super) fn take_failure(&self) -> Option<ClientFailure> {
        self.failure.lock().take()
    }

    fn fail(&self, code: &str, error: impl std::fmt::Display) {
        let mut failure = self.failure.lock();
        if failure.is_none() {
            *failure = Some(ClientFailure::new(
                "transport",
                code,
                error,
                self.dispatched(),
            ));
        }
    }
}

// TokioChildProcess's private AsyncRwTransport ignores malformed JSON and uses
// unbounded read_until. Use the SDK's fallible, bounded codec instead; the SDK
// service still owns all protocol negotiation, correlation and cancellation.
pub(super) struct ClientTransport<R, W> {
    reader: FramedRead<R, JsonRpcMessageCodec<RxJsonRpcMessage<RoleClient>>>,
    writer: Arc<tokio::sync::Mutex<Option<W>>>,
    state: TransportState,
}

impl<R: AsyncRead, W> ClientTransport<R, W> {
    pub(super) fn new(reader: R, writer: W, state: TransportState) -> Self {
        Self {
            reader: FramedRead::new(
                reader,
                JsonRpcMessageCodec::new_with_max_length(RESPONSE_MAX_BYTES),
            ),
            writer: Arc::new(tokio::sync::Mutex::new(Some(writer))),
            state,
        }
    }
}

impl<R, W> Transport<RoleClient> for ClientTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let writer = Arc::clone(&self.writer);
        let state = self.state.clone();
        async move {
            let call = matches!(&item, TxJsonRpcMessage::<RoleClient>::Request(request)
                if matches!(&request.request, ClientRequest::CallToolRequest(_)));
            let mut bytes = BytesMut::new();
            JsonRpcMessageCodec::new().encode(item, &mut bytes)?;
            if bytes.len() > REQUEST_MAX_BYTES {
                let error = io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "encoded MCP request exceeds 1 MiB",
                );
                state.fail("input_too_large", &error);
                return Err(error);
            }
            let mut writer = writer.lock().await;
            let writer = writer
                .as_mut()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "MCP pipe is closed"))?;
            // Once writing begins, even an IO failure may follow a partial dispatch.
            if call {
                state.dispatched.store(true, Ordering::Release);
            }
            writer.write_all(&bytes).await?;
            writer.flush().await
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        match self.reader.next().await {
            Some(Ok(message)) => Some(message),
            Some(Err(error)) => {
                let code = match &error {
                    JsonRpcMessageCodecError::MaxLineLengthExceeded => "output_too_large",
                    JsonRpcMessageCodecError::Io(_) => "disconnect",
                    _ => "protocol_error",
                };
                self.state.fail(code, error);
                None
            }
            None => None,
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        if let Some(mut writer) = self.writer.lock().await.take() {
            writer.shutdown().await?;
        }
        Ok(())
    }
}
