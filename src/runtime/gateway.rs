use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    control::{RuntimeReply, RuntimeRequest},
    control_rpc::protocol::ControlError,
};

pub(super) struct RuntimeCommand {
    pub(super) request: RuntimeRequest,
    pub(super) cancelled: CancellationToken,
    pub(super) reply: oneshot::Sender<Result<RuntimeReply, ControlError>>,
}

#[derive(Clone)]
pub(super) struct RuntimeControlClient {
    commands: mpsc::Sender<RuntimeCommand>,
    terminal_commands: mpsc::Sender<RuntimeCommand>,
}

pub(super) struct RuntimeTerminalPermit(mpsc::OwnedPermit<RuntimeCommand>);

pub(super) struct RuntimeControlReceiver {
    commands: mpsc::Receiver<RuntimeCommand>,
    terminal_commands: mpsc::Receiver<RuntimeCommand>,
}

pub(super) struct RuntimeGateway;

impl RuntimeGateway {
    pub(super) fn channel(capacity: usize) -> (RuntimeControlClient, RuntimeControlReceiver) {
        let (commands, receiver) = mpsc::channel(capacity);
        let (terminal_commands, terminal_receiver) = mpsc::channel(1);
        (
            RuntimeControlClient {
                commands,
                terminal_commands,
            },
            RuntimeControlReceiver {
                commands: receiver,
                terminal_commands: terminal_receiver,
            },
        )
    }
}

#[cfg(test)]
impl RuntimeControlClient {
    pub(super) fn available_terminal_delivery_permits(&self) -> usize {
        self.terminal_commands.capacity()
    }
}

impl RuntimeControlClient {
    pub(super) async fn request(
        &self,
        request: RuntimeRequest,
        cancelled: CancellationToken,
    ) -> Result<RuntimeReply, ControlError> {
        let sender = if matches!(
            &request,
            RuntimeRequest::FinalizeSettingsTransaction { .. }
                | RuntimeRequest::AbortSettingsTransaction { .. }
        ) {
            &self.terminal_commands
        } else {
            &self.commands
        };
        let permit = tokio::select! {
            permit = sender.reserve() => permit.map_err(|_| {
                ControlError::instance_unavailable("runtime command gateway is closed")
            })?,
            _ = cancelled.cancelled() => {
                return Err(ControlError::instance_unavailable(
                    "runtime command was cancelled before admission",
                ));
            }
        };
        let (reply, response) = oneshot::channel();
        permit.send(RuntimeCommand {
            request,
            cancelled: cancelled.clone(),
            reply,
        });
        tokio::select! {
            result = response => result.unwrap_or_else(|_| {
                Err(ControlError::instance_unavailable(
                    "runtime stopped before replying",
                ))
            }),
            _ = cancelled.cancelled() => Err(ControlError::instance_unavailable(
                "runtime command was cancelled",
            )),
        }
    }

    /// Admission may install authoritative state, so cancellation only applies
    /// before the command is queued. Once queued, its reply must be observed.
    pub(super) async fn request_admission(
        &self,
        request: RuntimeRequest,
        cancelled: CancellationToken,
    ) -> Result<RuntimeReply, ControlError> {
        let permit = tokio::select! {
            permit = self.commands.reserve() => permit.map_err(|_| {
                ControlError::instance_unavailable("runtime command gateway is closed")
            })?,
            _ = cancelled.cancelled() => {
                return Err(ControlError::instance_unavailable(
                    "runtime command was cancelled before admission",
                ));
            }
        };
        let (reply, response) = oneshot::channel();
        permit.send(RuntimeCommand {
            request,
            cancelled,
            reply,
        });
        response.await.unwrap_or_else(|_| {
            Err(ControlError::instance_unavailable(
                "runtime stopped before replying to transaction admission",
            ))
        })
    }

    pub(super) async fn reserve_terminal(&self) -> Result<RuntimeTerminalPermit, ControlError> {
        self.terminal_commands
            .clone()
            .reserve_owned()
            .await
            .map(RuntimeTerminalPermit)
            .map_err(|_| {
                ControlError::instance_unavailable("runtime terminal command gateway is closed")
            })
    }

    pub(super) async fn request_with_terminal_permit(
        &self,
        permit: RuntimeTerminalPermit,
        request: RuntimeRequest,
    ) -> Result<RuntimeReply, ControlError> {
        debug_assert!(matches!(
            request,
            RuntimeRequest::FinalizeSettingsTransaction { .. }
                | RuntimeRequest::AbortSettingsTransaction { .. }
        ));
        let (reply, response) = oneshot::channel();
        permit.0.send(RuntimeCommand {
            request,
            cancelled: CancellationToken::new(),
            reply,
        });
        response.await.unwrap_or_else(|_| {
            Err(ControlError::instance_unavailable(
                "runtime stopped before terminal transaction delivery",
            ))
        })
    }
}

impl RuntimeControlReceiver {
    pub(super) async fn recv(&mut self) -> Option<RuntimeCommand> {
        if self.terminal_commands.is_closed() {
            return self.commands.recv().await;
        }
        if self.commands.is_closed() {
            return self.terminal_commands.recv().await;
        }
        tokio::select! {
            biased;
            command = self.terminal_commands.recv() => command,
            command = self.commands.recv() => command,
        }
    }

    pub(super) fn try_recv(&mut self) -> Result<RuntimeCommand, mpsc::error::TryRecvError> {
        match self.terminal_commands.try_recv() {
            Ok(command) => Ok(command),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                self.commands.try_recv()
            }
        }
    }

    pub(super) fn close(&mut self) {
        self.terminal_commands.close();
        self.commands.close();
    }

    #[cfg(test)]
    pub(super) fn is_closed(&self) -> bool {
        self.terminal_commands.is_closed() && self.commands.is_closed()
    }
}
