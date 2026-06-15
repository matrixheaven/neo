use std::process::Stdio;

use neo_sdk::{JsonlCodec, RpcMessage, RpcOutcome, RpcRequest, RpcResponse};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use crate::{ExtensionEnv, ExtensionTransport};

#[derive(Debug, thiserror::Error)]
pub enum ExtensionRunnerError {
    #[error("failed to spawn extension command {command:?}: {source}")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    #[error("extension command did not expose stdin")]
    MissingStdin,
    #[error("extension command did not expose stdout")]
    MissingStdout,
    #[error("failed to write RPC frame to extension: {0}")]
    Write(std::io::Error),
    #[error("failed to read RPC frame from extension: {0}")]
    Read(std::io::Error),
    #[error("extension closed stdout before sending a response")]
    Eof,
    #[error("failed to decode extension RPC frame: {0}")]
    Decode(#[from] neo_sdk::RpcCodecError),
    #[error("expected RPC response, received {0:?}")]
    UnexpectedMessage(RpcMessage),
    #[error("response id mismatch: expected {expected:?}, got {actual:?}")]
    ResponseIdMismatch { expected: String, actual: String },
    #[error("extension returned RPC error {code:?}: {message}")]
    RpcFailure {
        code: neo_sdk::RpcErrorCode,
        message: String,
    },
}

pub struct ExtensionRunner {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ExtensionRunner {
    pub fn spawn(transport: ExtensionTransport) -> Result<Self, ExtensionRunnerError> {
        match transport {
            ExtensionTransport::Stdio { command, args, env } => spawn_stdio(command, args, env),
        }
    }

    pub async fn request(
        &mut self,
        request: RpcRequest,
    ) -> Result<RpcResponse, ExtensionRunnerError> {
        let expected_id = request.id.clone();
        let frame = JsonlCodec::encode(&RpcMessage::Request(request))?;
        self.stdin
            .write_all(frame.as_bytes())
            .await
            .map_err(ExtensionRunnerError::Write)?;
        self.stdin
            .flush()
            .await
            .map_err(ExtensionRunnerError::Write)?;

        let response = loop {
            let mut line = String::new();
            let bytes = self
                .stdout
                .read_line(&mut line)
                .await
                .map_err(ExtensionRunnerError::Read)?;
            if bytes == 0 {
                return Err(ExtensionRunnerError::Eof);
            }
            match JsonlCodec::decode_line(&line)? {
                RpcMessage::Response(response) => break response,
                RpcMessage::Notification(_) => {}
                RpcMessage::Request(request) => {
                    return Err(ExtensionRunnerError::UnexpectedMessage(
                        RpcMessage::Request(request),
                    ));
                }
            }
        };
        if response.id != expected_id {
            return Err(ExtensionRunnerError::ResponseIdMismatch {
                expected: expected_id,
                actual: response.id,
            });
        }
        if let RpcOutcome::Failure { error } = &response.outcome {
            return Err(ExtensionRunnerError::RpcFailure {
                code: error.code,
                message: error.message.clone(),
            });
        }
        Ok(response)
    }

    #[must_use]
    pub fn child_id(&self) -> Option<u32> {
        self.child.id()
    }
}

impl Drop for ExtensionRunner {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn spawn_stdio(
    command: String,
    args: Vec<String>,
    env: Vec<ExtensionEnv>,
) -> Result<ExtensionRunner, ExtensionRunnerError> {
    let mut process = Command::new(&command);
    process
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for ExtensionEnv { name, value } in env {
        process.env(name, value);
    }

    let mut child = process
        .spawn()
        .map_err(|source| ExtensionRunnerError::Spawn { command, source })?;
    let stdin = child
        .stdin
        .take()
        .ok_or(ExtensionRunnerError::MissingStdin)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(ExtensionRunnerError::MissingStdout)?;

    Ok(ExtensionRunner {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}
