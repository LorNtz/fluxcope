use rmcp::{
    ErrorData, RoleServer,
    model::{ClientNotification, ProtocolVersion, RequestId},
    service::{NotificationContext, RequestContext, RxJsonRpcMessage, Service, TxJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use std::{
    borrow::Cow,
    collections::HashMap,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncBufRead, AsyncRead, AsyncWrite, BufReader, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError},
};

pub(super) const MCP_STDIO_REQUEST_MAX_BYTES: usize = 1024 * 1024;
pub(super) const MCP_STDIO_RESPONSE_MAX_BYTES: usize = 8 * 1024 * 1024;
const MCP_STDIO_IN_FLIGHT_RESPONSES: usize = 4;

pub(super) fn session<S, R, W>(
    service: S,
    reader: R,
    writer: W,
    cancelled: tokio_util::sync::CancellationToken,
) -> (
    impl Service<RoleServer>,
    impl Transport<RoleServer, Error = io::Error> + 'static,
)
where
    S: Service<RoleServer>,
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let tracker = ResponseTracker::new(MCP_STDIO_IN_FLIGHT_RESPONSES);
    let reader = BoundedLineReader::new(reader, MCP_STDIO_REQUEST_MAX_BYTES);
    let writer = BoundedLineWriter::new(writer, MCP_STDIO_RESPONSE_MAX_BYTES);
    let transport = cancel_on_disconnect(
        ResponseBoundedTransport::new(
            AsyncRwTransport::new_server(reader, writer),
            Arc::clone(&tracker),
        ),
        cancelled,
    );
    (
        CompletionTrackedService {
            inner: service,
            tracker,
        },
        transport,
    )
}

struct BoundedLineReader<R> {
    inner: BufReader<R>,
    line_bytes: usize,
    max_line_bytes: usize,
}

impl<R: AsyncRead> BoundedLineReader<R> {
    fn new(inner: R, max_line_bytes: usize) -> Self {
        Self {
            inner: BufReader::new(inner),
            line_bytes: 0,
            max_line_bytes,
        }
    }
}

impl<R> AsyncRead for BoundedLineReader<R>
where
    R: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let available = ready!(Pin::new(&mut this.inner).poll_fill_buf(cx))?;
        if available.is_empty() {
            return Poll::Ready(Ok(()));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let content_bytes = newline.unwrap_or(available.len());
        if this.line_bytes.saturating_add(content_bytes) > this.max_line_bytes {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP stdio request line exceeds the fixed byte limit",
            )));
        }
        let segment_bytes = newline.map_or(available.len(), |index| index + 1);
        let copied = segment_bytes.min(output.remaining());
        output.put_slice(&available[..copied]);
        Pin::new(&mut this.inner).consume(copied);
        if newline.is_some_and(|index| copied > index) {
            this.line_bytes = 0;
        } else {
            this.line_bytes = this.line_bytes.saturating_add(copied);
        }
        Poll::Ready(Ok(()))
    }
}

struct BoundedLineWriter<W> {
    inner: W,
    line_bytes: usize,
    max_line_bytes: usize,
}

impl<W> BoundedLineWriter<W> {
    fn new(inner: W, max_line_bytes: usize) -> Self {
        Self {
            inner,
            line_bytes: 0,
            max_line_bytes,
        }
    }

    fn validate(&self, bytes: &[u8]) -> io::Result<()> {
        let mut line_bytes = self.line_bytes;
        for segment in bytes.split_inclusive(|byte| *byte == b'\n') {
            let terminated = segment.last() == Some(&b'\n');
            let content_bytes = segment.len().saturating_sub(usize::from(terminated));
            line_bytes = line_bytes.saturating_add(content_bytes);
            if line_bytes > self.max_line_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "MCP stdio response line exceeds the fixed byte limit",
                ));
            }
            if terminated {
                line_bytes = 0;
            }
        }
        Ok(())
    }

    fn account(&mut self, bytes: &[u8]) {
        for segment in bytes.split_inclusive(|byte| *byte == b'\n') {
            if segment.last() == Some(&b'\n') {
                self.line_bytes = 0;
            } else {
                self.line_bytes = self.line_bytes.saturating_add(segment.len());
            }
        }
    }
}

impl<W> AsyncWrite for BoundedLineWriter<W>
where
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.validate(bytes)?;
        match Pin::new(&mut this.inner).poll_write(cx, bytes) {
            Poll::Ready(Ok(written)) => {
                this.account(&bytes[..written]);
                Poll::Ready(Ok(written))
            }
            result => result,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

enum AdmissionResult {
    Admitted,
    Saturated,
    Rejected,
}

struct TrackedRequest {
    permit: OwnedSemaphorePermit,
    cancelled: bool,
    handler_done: bool,
}

struct ResponseTracker {
    admission: Arc<Semaphore>,
    pending: Mutex<HashMap<RequestId, TrackedRequest>>,
}

impl ResponseTracker {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            admission: Arc::new(Semaphore::new(limit)),
            pending: Mutex::new(HashMap::with_capacity(limit)),
        })
    }

    fn try_admit(&self, request_id: RequestId) -> AdmissionResult {
        let permit = match Arc::clone(&self.admission).try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::NoPermits) => return AdmissionResult::Saturated,
            Err(TryAcquireError::Closed) => return AdmissionResult::Rejected,
        };
        if self.track(request_id, permit) {
            AdmissionResult::Admitted
        } else {
            AdmissionResult::Rejected
        }
    }

    fn track(&self, request_id: RequestId, permit: OwnedSemaphorePermit) -> bool {
        let mut pending = self.pending.lock().expect("MCP response admission lock");
        if pending.contains_key(&request_id) {
            return false;
        }
        pending.insert(
            request_id,
            TrackedRequest {
                permit,
                cancelled: false,
                handler_done: false,
            },
        );
        true
    }

    fn mark_cancelled(&self, request_id: &RequestId) {
        let mut pending = self.pending.lock().expect("MCP response admission lock");
        let remove = pending.get_mut(request_id).is_some_and(|tracked| {
            tracked.cancelled = true;
            tracked.handler_done
        });
        if remove {
            pending.remove(request_id);
        }
    }

    fn mark_handler_done(&self, request_id: &RequestId) {
        let mut pending = self.pending.lock().expect("MCP response admission lock");
        let remove = pending.get_mut(request_id).is_some_and(|tracked| {
            tracked.handler_done = true;
            tracked.cancelled
        });
        if remove {
            pending.remove(request_id);
        }
    }

    fn take_for_response(&self, request_id: &RequestId) -> Option<OwnedSemaphorePermit> {
        self.pending
            .lock()
            .expect("MCP response admission lock")
            .remove(request_id)
            .map(|tracked| tracked.permit)
    }

    fn clear(&self) {
        self.pending
            .lock()
            .expect("MCP response admission lock")
            .clear();
    }
}

struct HandlerCompletionGuard {
    tracker: Arc<ResponseTracker>,
    request_id: RequestId,
}

impl Drop for HandlerCompletionGuard {
    fn drop(&mut self) {
        self.tracker.mark_handler_done(&self.request_id);
    }
}

struct CompletionTrackedService<S> {
    inner: S,
    tracker: Arc<ResponseTracker>,
}

impl<S> Service<RoleServer> for CompletionTrackedService<S>
where
    S: Service<RoleServer>,
{
    async fn handle_request(
        &self,
        request: <RoleServer as rmcp::service::ServiceRole>::PeerReq,
        context: RequestContext<RoleServer>,
    ) -> Result<<RoleServer as rmcp::service::ServiceRole>::Resp, ErrorData> {
        let _completion = HandlerCompletionGuard {
            tracker: Arc::clone(&self.tracker),
            request_id: context.id.clone(),
        };
        self.inner.handle_request(request, context).await
    }

    async fn handle_notification(
        &self,
        notification: <RoleServer as rmcp::service::ServiceRole>::PeerNot,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.inner.handle_notification(notification, context).await
    }

    fn get_info(&self) -> <RoleServer as rmcp::service::ServiceRole>::Info {
        self.inner.get_info()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        self.inner.supported_protocol_versions()
    }
}

fn cancellation_request_id(message: &RxJsonRpcMessage<RoleServer>) -> Option<&RequestId> {
    match message {
        RxJsonRpcMessage::<RoleServer>::Notification(notification) => {
            match &notification.notification {
                ClientNotification::CancelledNotification(cancelled) => {
                    cancelled.params.request_id.as_ref()
                }
                _ => None,
            }
        }
        _ => None,
    }
}

struct ResponseBoundedTransport<T> {
    inner: T,
    tracker: Arc<ResponseTracker>,
    buffered: Option<RxJsonRpcMessage<RoleServer>>,
}

impl<T> ResponseBoundedTransport<T> {
    fn new(inner: T, tracker: Arc<ResponseTracker>) -> Self {
        Self {
            inner,
            tracker,
            buffered: None,
        }
    }
}

pub(super) fn cancel_on_disconnect<T>(
    inner: T,
    cancelled: tokio_util::sync::CancellationToken,
) -> impl Transport<RoleServer, Error = T::Error>
where
    T: Transport<RoleServer>,
{
    DisconnectCancellingTransport { inner, cancelled }
}

struct DisconnectCancellingTransport<T> {
    inner: T,
    cancelled: tokio_util::sync::CancellationToken,
}

impl<T> Transport<RoleServer> for DisconnectCancellingTransport<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleServer>>> + Send {
        let cancelled = self.cancelled.clone();
        let receive = self.inner.receive();
        async move {
            let message = receive.await;
            if message.is_none() {
                cancelled.cancel();
            }
            message
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

impl<T> Transport<RoleServer> for ResponseBoundedTransport<T>
where
    T: Transport<RoleServer> + 'static,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let response_id = match &item {
            TxJsonRpcMessage::<RoleServer>::Response(response) => Some(response.id.clone()),
            TxJsonRpcMessage::<RoleServer>::Error(error) => error.id.clone(),
            _ => None,
        };
        let response_permit = response_id
            .as_ref()
            .and_then(|request_id| self.tracker.take_for_response(request_id));
        let send = self.inner.send(item);
        async move {
            let result = send.await;
            drop(response_permit);
            result
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        if self.buffered.is_none() {
            self.buffered = self.inner.receive().await;
        }
        if let Some(cancelled_id) = cancellation_request_id(self.buffered.as_ref()?) {
            self.tracker.mark_cancelled(cancelled_id);
        }
        let request_id = match self.buffered.as_ref()? {
            RxJsonRpcMessage::<RoleServer>::Request(request) => request.id.clone(),
            _ => return self.buffered.take(),
        };
        match self.tracker.try_admit(request_id.clone()) {
            AdmissionResult::Admitted => return self.buffered.take(),
            AdmissionResult::Rejected => {
                self.buffered.take();
                return None;
            }
            AdmissionResult::Saturated => {}
        }

        let admission = Arc::clone(&self.tracker.admission);
        tokio::select! {
            permit = admission.acquire_owned() => {
                let Ok(permit) = permit else {
                    self.buffered.take();
                    return None;
                };
                if self.tracker.track(request_id, permit) {
                    self.buffered.take()
                } else {
                    self.buffered.take();
                    None
                }
            }
            message = self.inner.receive() => {
                match message {
                    Some(RxJsonRpcMessage::<RoleServer>::Request(_)) | None => {
                        self.buffered.take();
                        None
                    }
                    Some(message) => {
                        let cancels_buffered = cancellation_request_id(&message)
                            .is_some_and(|cancelled_id| {
                                self.tracker.mark_cancelled(cancelled_id);
                                cancelled_id == &request_id
                            });
                        if cancels_buffered {
                            self.buffered.take();
                        }
                        Some(message)
                    }
                }
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let close = self.inner.close();
        let tracker = Arc::clone(&self.tracker);
        async move {
            let result = close.await;
            tracker.clear();
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::{
        ServiceExt,
        model::{
            ClientCapabilities, ClientInfo, ClientRequest, Implementation, PingRequest, RequestId,
            ServerInfo, ServerResult,
        },
        service::PeerRequestOptions,
    };
    use std::collections::VecDeque as Queue;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        sync::Notify,
    };

    #[derive(Clone)]
    struct CancellationAwareService {
        started: Arc<Notify>,
    }

    impl Service<RoleServer> for CancellationAwareService {
        async fn handle_request(
            &self,
            request: ClientRequest,
            context: RequestContext<RoleServer>,
        ) -> Result<ServerResult, ErrorData> {
            if matches!(request, ClientRequest::InitializeRequest(_)) {
                return Ok(ServerResult::InitializeResult(self.get_info()));
            }
            self.started.notify_one();
            context.ct.cancelled().await;
            Err(ErrorData::internal_error("request cancelled", None))
        }

        async fn handle_notification(
            &self,
            _notification: ClientNotification,
            _context: NotificationContext<RoleServer>,
        ) -> Result<(), ErrorData> {
            Ok(())
        }

        fn get_info(&self) -> ServerInfo {
            ServerInfo::default()
        }
    }

    #[tokio::test]
    async fn request_reader_rejects_a_line_before_it_exceeds_the_cap() {
        let (mut input, source) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move {
            input.write_all(b"123456789\n").await.expect("test input");
        });
        let mut bounded = BoundedLineReader::new(source, 8);
        let mut received = Vec::new();

        let error = bounded
            .read_to_end(&mut received)
            .await
            .expect_err("oversized input line");

        writer.await.expect("input writer");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(received.is_empty());
    }

    #[tokio::test]
    async fn response_writer_rejects_an_oversized_line_without_partial_output() {
        let (sink, mut output) = tokio::io::duplex(64);
        let mut bounded = BoundedLineWriter::new(sink, 8);

        let error = bounded
            .write_all(b"123456789\n")
            .await
            .expect_err("oversized output line");
        drop(bounded);
        let mut received = Vec::new();
        output
            .read_to_end(&mut received)
            .await
            .expect("test output");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(received.is_empty());
    }

    struct GatedTransport {
        incoming: Queue<RxJsonRpcMessage<RoleServer>>,
        send_gate: Arc<Semaphore>,
    }

    impl Transport<RoleServer> for GatedTransport {
        type Error = io::Error;

        fn send(
            &mut self,
            _item: TxJsonRpcMessage<RoleServer>,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
            let gate = Arc::clone(&self.send_gate);
            async move {
                let _permit = gate
                    .acquire()
                    .await
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "send gate closed"))?;
                Ok(())
            }
        }

        async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
            self.incoming.pop_front()
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn incoming_request(id: i64) -> RxJsonRpcMessage<RoleServer> {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "ping"
        }))
        .expect("incoming request")
    }

    fn incoming_cancellation(id: i64) -> RxJsonRpcMessage<RoleServer> {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {
                "requestId": id,
                "reason": "test cancellation"
            }
        }))
        .expect("incoming cancellation")
    }

    fn outgoing_response(id: i64) -> TxJsonRpcMessage<RoleServer> {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        }))
        .expect("outgoing response")
    }

    fn outgoing_cancelled_error(id: i64) -> TxJsonRpcMessage<RoleServer> {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32800,
                "message": "Request cancelled"
            }
        }))
        .expect("outgoing cancellation error")
    }

    #[tokio::test]
    async fn response_admission_is_held_until_the_matching_write_finishes() {
        let gate = Arc::new(Semaphore::new(0));
        let tracker = ResponseTracker::new(1);
        let inner = GatedTransport {
            incoming: Queue::from([incoming_request(1)]),
            send_gate: Arc::clone(&gate),
        };
        let mut transport = ResponseBoundedTransport::new(inner, Arc::clone(&tracker));
        assert!(matches!(
            transport.receive().await,
            Some(RxJsonRpcMessage::<RoleServer>::Request(request)) if request.id == RequestId::Number(1)
        ));
        let send = tokio::spawn(transport.send(outgoing_response(1)));

        assert!(
            tracker.admission.clone().try_acquire_owned().is_err(),
            "the matching response must hold admission while its write is blocked"
        );
        gate.add_permits(1);
        send.await.expect("send task").expect("response send");
        assert!(
            tracker.admission.clone().try_acquire_owned().is_ok(),
            "the completed response write must release admission"
        );
    }

    #[tokio::test]
    async fn cancelled_request_admission_is_held_until_the_error_response_write_finishes() {
        let gate = Arc::new(Semaphore::new(0));
        let tracker = ResponseTracker::new(1);
        assert!(matches!(
            tracker.try_admit(RequestId::Number(1)),
            AdmissionResult::Admitted
        ));
        let mut transport = ResponseBoundedTransport::new(
            GatedTransport {
                incoming: Queue::new(),
                send_gate: Arc::clone(&gate),
            },
            Arc::clone(&tracker),
        );
        let send = tokio::spawn(transport.send(outgoing_cancelled_error(1)));

        assert!(
            tracker.admission.clone().try_acquire_owned().is_err(),
            "cancellation error admission must remain held while its write is blocked"
        );
        gate.add_permits(1);
        send.await
            .expect("send task")
            .expect("cancellation response");
        assert!(
            tracker.admission.clone().try_acquire_owned().is_ok(),
            "the completed cancellation response write must release admission"
        );
    }

    #[tokio::test]
    async fn saturated_reader_discards_a_buffered_request_cancelled_before_dispatch() {
        let gate = Arc::new(Semaphore::new(0));
        let tracker = ResponseTracker::new(1);
        let inner = GatedTransport {
            incoming: Queue::from([
                incoming_request(1),
                incoming_request(2),
                incoming_cancellation(2),
            ]),
            send_gate: Arc::clone(&gate),
        };
        let mut transport = ResponseBoundedTransport::new(inner, Arc::clone(&tracker));
        assert!(matches!(
            transport.receive().await,
            Some(RxJsonRpcMessage::<RoleServer>::Request(request)) if request.id == RequestId::Number(1)
        ));

        let notification =
            tokio::time::timeout(std::time::Duration::from_millis(25), transport.receive())
                .await
                .expect("saturated reader must continue reading");
        assert!(matches!(
            notification,
            Some(RxJsonRpcMessage::<RoleServer>::Notification(_))
        ));
        assert!(
            tracker.admission.clone().try_acquire_owned().is_err(),
            "forwarding cancellation must not release the pending response permit"
        );
        let send = tokio::spawn(transport.send(outgoing_response(1)));
        gate.add_permits(1);
        send.await.expect("send task").expect("response send");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), transport.receive())
                .await
                .expect("reader must finish without dispatching the cancelled buffered request")
                .is_none()
        );
    }

    #[tokio::test]
    async fn second_saturated_request_closes_the_session_without_blocking() {
        let inner = GatedTransport {
            incoming: Queue::from([
                incoming_request(1),
                incoming_request(2),
                incoming_request(3),
            ]),
            send_gate: Arc::new(Semaphore::new(0)),
        };
        let mut transport = ResponseBoundedTransport::new(inner, ResponseTracker::new(1));
        assert!(transport.receive().await.is_some());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), transport.receive())
                .await
                .expect("a second saturated request must not block the read loop")
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_rmcp_cancellations_restore_all_response_admission() {
        let tracker = ResponseTracker::new(4);
        let started = Arc::new(Notify::new());
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        let (server_reader, server_writer) = tokio::io::split(server_stream);
        let transport = ResponseBoundedTransport::new(
            AsyncRwTransport::new_server(
                BoundedLineReader::new(server_reader, MCP_STDIO_REQUEST_MAX_BYTES),
                BoundedLineWriter::new(server_writer, MCP_STDIO_RESPONSE_MAX_BYTES),
            ),
            Arc::clone(&tracker),
        );
        let service = CompletionTrackedService {
            inner: CancellationAwareService {
                started: Arc::clone(&started),
            },
            tracker: Arc::clone(&tracker),
        };
        let server = tokio::spawn(async move {
            let running = service.serve(transport).await.expect("serve test service");
            running.waiting().await.expect("test service shutdown");
        });
        let client = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            ClientInfo::new(
                ClientCapabilities::default(),
                Implementation::new("stdio-cancellation-test", "1.0"),
            )
            .serve(client_stream),
        )
        .await
        .expect("initialize test client within deadline")
        .expect("initialize test client");

        for _ in 0..8 {
            let request = client
                .send_cancellable_request(
                    ClientRequest::PingRequest(PingRequest::default()),
                    PeerRequestOptions::no_options(),
                )
                .await
                .expect("send cancellable request");
            tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
                .await
                .expect("handler started");
            request
                .cancel(Some("test cancellation".to_owned()))
                .await
                .expect("cancel request");
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while tracker.admission.available_permits() != 4 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("cancelled handler restores response admission");
        }

        tokio::time::timeout(std::time::Duration::from_secs(1), client.cancel())
            .await
            .expect("close test client within deadline")
            .expect("close test client");
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("server task stops within deadline")
            .expect("server task");
        assert_eq!(tracker.admission.available_permits(), 4);
    }

    #[tokio::test]
    async fn duplicate_outstanding_request_ids_are_rejected_without_replacing_admission() {
        let tracker = ResponseTracker::new(2);
        let request_id = RequestId::Number(1);
        assert!(matches!(
            tracker.try_admit(request_id.clone()),
            AdmissionResult::Admitted
        ));
        assert!(matches!(
            tracker.try_admit(request_id),
            AdmissionResult::Rejected
        ));
        assert_eq!(tracker.pending.lock().expect("pending responses").len(), 1);
        assert_eq!(tracker.admission.available_permits(), 1);
    }
}
