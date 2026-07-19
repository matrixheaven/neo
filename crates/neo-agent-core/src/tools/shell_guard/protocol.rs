use std::{io, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{GuardLimits, output::StreamKind, status::GuardExit};

pub(crate) const MAX_FRAME_BODY: usize = 1024 * 1024;
pub(crate) const MAX_TERMINAL_WRITE: usize = 64 * 1024;
const FRAME_HEADER_LEN: usize = 9;
const FRAGMENT_HEADER_LEN: usize = 5;

const START: u8 = 1;
const WRITE: u8 = 2;
const READ: u8 = 3;
const RESIZE: u8 = 4;
// Frame kind 5 was SetBackgroundDeadline and has been removed.
const STOP: u8 = 6;
const STARTED: u8 = 101;
const OUTPUT: u8 = 102;
const ACK: u8 = 103;
const BUSY: u8 = 104;
const SNAPSHOT: u8 = 105;
const EXITED: u8 = 106;
const ERROR: u8 = 107;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GuardTaskKind {
    Bash,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StartRequest {
    pub(crate) task_id: String,
    pub(crate) kind: GuardTaskKind,
    pub(crate) command: String,
    pub(crate) limits: GuardLimits,
    pub(crate) status_dir: PathBuf,
    pub(crate) cols: Option<u16>,
    pub(crate) rows: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardRequest {
    Start {
        request_id: u64,
        request: StartRequest,
    },
    Write {
        request_id: u64,
        data: Vec<u8>,
    },
    Read {
        request_id: u64,
        offset: u64,
        max_bytes: usize,
    },
    Resize {
        request_id: u64,
        cols: u16,
        rows: u16,
    },
    Stop {
        request_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardResponse {
    Started {
        request_id: u64,
        guardian_pid: u32,
        command_pid: u32,
        command_start_id: u64,
    },
    Output {
        stream: StreamKind,
        data: Vec<u8>,
    },
    Ack {
        request_id: u64,
    },
    Busy {
        request_id: u64,
    },
    Snapshot {
        request_id: u64,
        offset: u64,
        total: u64,
        discarded: u64,
        data: Vec<u8>,
    },
    Exited {
        exit: GuardExit,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Error {
        request_id: u64,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GuardResponsePart {
    Complete(GuardResponse),
    Fragment {
        kind: u8,
        request_id: u64,
        sequence: u32,
        final_fragment: bool,
        data: Vec<u8>,
    },
}

#[derive(Debug, Error)]
pub(crate) enum ProtocolError {
    #[error("guard frame body is {size} bytes; maximum is {max}")]
    FrameTooLarge { size: usize, max: usize },
    #[error("unknown guard frame kind {0:#x}")]
    UnknownKind(u8),
    #[error("truncated guard frame")]
    Truncated,
    #[error("invalid guard frame: {0}")]
    Invalid(&'static str),
    #[error("guard protocol I/O: {0}")]
    Io(#[from] io::Error),
    #[error("guard protocol JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("guard protocol UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub(crate) async fn read_request<R>(reader: &mut R) -> Result<GuardRequest, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let frame = read_frame(reader).await?;
    decode_request_body(&frame)
}

pub(crate) fn request_stream<R>(
    reader: R,
) -> impl futures::Stream<Item = Result<GuardRequest, ProtocolError>>
where
    R: AsyncRead + Unpin,
{
    futures::stream::unfold(reader, |mut reader| async move {
        let request = read_request(&mut reader).await;
        Some((request, reader))
    })
}

pub(crate) async fn write_request<W>(
    writer: &mut W,
    request: &GuardRequest,
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(&encode_request(request)?).await?;
    writer.flush().await?;
    Ok(())
}

pub(super) async fn read_response<R>(reader: &mut R) -> Result<GuardResponsePart, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let frame = read_frame(reader).await?;
    decode_response_body(&frame)
}

pub(crate) async fn write_response<W>(
    writer: &mut W,
    response: &GuardResponse,
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    for frame in encode_response_frames(response)? {
        writer.write_all(&frame).await?;
    }
    writer.flush().await?;
    Ok(())
}

async fn read_frame<R>(reader: &mut R) -> Result<Vec<u8>, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let size = reader.read_u32().await? as usize;
    ensure_frame_size(size)?;
    let mut body = vec![0; size];
    reader.read_exact(&mut body).await.map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            ProtocolError::Truncated
        } else {
            ProtocolError::Io(error)
        }
    })?;
    Ok(body)
}

fn encode_request(request: &GuardRequest) -> Result<Vec<u8>, ProtocolError> {
    let (kind, request_id, payload) = match request {
        GuardRequest::Start {
            request_id,
            request,
        } => (START, *request_id, serde_json::to_vec(request)?),
        GuardRequest::Write { request_id, data } => {
            if data.len() > MAX_TERMINAL_WRITE {
                return Err(ProtocolError::FrameTooLarge {
                    size: data.len(),
                    max: MAX_TERMINAL_WRITE,
                });
            }
            (WRITE, *request_id, data.clone())
        }
        GuardRequest::Read {
            request_id,
            offset,
            max_bytes,
        } => {
            let mut payload = Vec::with_capacity(16);
            payload.extend_from_slice(&offset.to_be_bytes());
            payload.extend_from_slice(&usize_to_u64(*max_bytes)?.to_be_bytes());
            (READ, *request_id, payload)
        }
        GuardRequest::Resize {
            request_id,
            cols,
            rows,
        } => {
            let mut payload = Vec::with_capacity(4);
            payload.extend_from_slice(&cols.to_be_bytes());
            payload.extend_from_slice(&rows.to_be_bytes());
            (RESIZE, *request_id, payload)
        }
        GuardRequest::Stop { request_id } => (STOP, *request_id, Vec::new()),
    };
    encode_frame(kind, request_id, &payload)
}

fn decode_request_body(body: &[u8]) -> Result<GuardRequest, ProtocolError> {
    let (kind, request_id, payload) = split_body(body)?;
    match kind {
        START => Ok(GuardRequest::Start {
            request_id,
            request: serde_json::from_slice(payload)?,
        }),
        WRITE if payload.len() > MAX_TERMINAL_WRITE => Err(ProtocolError::FrameTooLarge {
            size: payload.len(),
            max: MAX_TERMINAL_WRITE,
        }),
        WRITE => Ok(GuardRequest::Write {
            request_id,
            data: payload.to_vec(),
        }),
        READ => {
            require_len(payload, 16)?;
            Ok(GuardRequest::Read {
                request_id,
                offset: u64::from_be_bytes(
                    payload[0..8]
                        .try_into()
                        .map_err(|_| ProtocolError::Truncated)?,
                ),
                max_bytes: u64_to_usize(u64::from_be_bytes(
                    payload[8..16]
                        .try_into()
                        .map_err(|_| ProtocolError::Truncated)?,
                ))?,
            })
        }
        RESIZE => {
            require_len(payload, 4)?;
            Ok(GuardRequest::Resize {
                request_id,
                cols: u16::from_be_bytes([payload[0], payload[1]]),
                rows: u16::from_be_bytes([payload[2], payload[3]]),
            })
        }
        STOP => {
            require_len(payload, 0)?;
            Ok(GuardRequest::Stop { request_id })
        }
        other => Err(ProtocolError::UnknownKind(other)),
    }
}

fn encode_response_frames(response: &GuardResponse) -> Result<Vec<Vec<u8>>, ProtocolError> {
    let (kind, request_id, payload, fragmented) = match response {
        GuardResponse::Started {
            request_id,
            guardian_pid,
            command_pid,
            command_start_id,
        } => {
            let mut payload = Vec::with_capacity(16);
            payload.extend_from_slice(&guardian_pid.to_be_bytes());
            payload.extend_from_slice(&command_pid.to_be_bytes());
            payload.extend_from_slice(&command_start_id.to_be_bytes());
            (STARTED, *request_id, payload, false)
        }
        GuardResponse::Output { stream, data } => {
            let mut payload = Vec::with_capacity(data.len() + 1);
            payload.push(match stream {
                StreamKind::Stdout => 0,
                StreamKind::Stderr => 1,
            });
            payload.extend_from_slice(data);
            (OUTPUT, 0, payload, false)
        }
        GuardResponse::Ack { request_id } => (ACK, *request_id, Vec::new(), false),
        GuardResponse::Busy { request_id } => (BUSY, *request_id, Vec::new(), false),
        GuardResponse::Snapshot {
            request_id,
            offset,
            total,
            discarded,
            data,
        } => {
            let mut payload = Vec::with_capacity(24 + data.len());
            payload.extend_from_slice(&offset.to_be_bytes());
            payload.extend_from_slice(&total.to_be_bytes());
            payload.extend_from_slice(&discarded.to_be_bytes());
            payload.extend_from_slice(data);
            (SNAPSHOT, *request_id, payload, true)
        }
        GuardResponse::Exited {
            exit,
            stdout,
            stderr,
        } => {
            let metadata = serde_json::to_vec(exit)?;
            let mut payload = Vec::with_capacity(12 + metadata.len() + stdout.len() + stderr.len());
            payload.extend_from_slice(&usize_to_u32(metadata.len())?.to_be_bytes());
            payload.extend_from_slice(&usize_to_u32(stdout.len())?.to_be_bytes());
            payload.extend_from_slice(&usize_to_u32(stderr.len())?.to_be_bytes());
            payload.extend_from_slice(&metadata);
            payload.extend_from_slice(stdout);
            payload.extend_from_slice(stderr);
            (EXITED, 0, payload, true)
        }
        GuardResponse::Error {
            request_id,
            message,
        } => (ERROR, *request_id, message.as_bytes().to_vec(), false),
    };
    if !fragmented {
        return Ok(vec![encode_frame(kind, request_id, &payload)?]);
    }

    let chunk_size = MAX_FRAME_BODY - FRAME_HEADER_LEN - FRAGMENT_HEADER_LEN;
    payload
        .chunks(chunk_size)
        .enumerate()
        .map(|(sequence, data)| {
            let mut fragment = Vec::with_capacity(FRAGMENT_HEADER_LEN + data.len());
            fragment.extend_from_slice(&usize_to_u32(sequence)?.to_be_bytes());
            fragment.push(u8::from(sequence + 1 == payload.len().div_ceil(chunk_size)));
            fragment.extend_from_slice(data);
            encode_frame(kind, request_id, &fragment)
        })
        .collect()
}

fn decode_response_body(body: &[u8]) -> Result<GuardResponsePart, ProtocolError> {
    let (kind, request_id, payload) = split_body(body)?;
    let response = match kind {
        STARTED => {
            require_len(payload, 16)?;
            GuardResponse::Started {
                request_id,
                guardian_pid: u32::from_be_bytes(
                    payload[0..4]
                        .try_into()
                        .map_err(|_| ProtocolError::Truncated)?,
                ),
                command_pid: u32::from_be_bytes(
                    payload[4..8]
                        .try_into()
                        .map_err(|_| ProtocolError::Truncated)?,
                ),
                command_start_id: u64::from_be_bytes(
                    payload[8..16]
                        .try_into()
                        .map_err(|_| ProtocolError::Truncated)?,
                ),
            }
        }
        OUTPUT => {
            let (&stream, data) = payload.split_first().ok_or(ProtocolError::Truncated)?;
            let stream = match stream {
                0 => StreamKind::Stdout,
                1 => StreamKind::Stderr,
                _ => return Err(ProtocolError::Invalid("invalid output stream")),
            };
            GuardResponse::Output {
                stream,
                data: data.to_vec(),
            }
        }
        ACK => {
            require_len(payload, 0)?;
            GuardResponse::Ack { request_id }
        }
        BUSY => {
            require_len(payload, 0)?;
            GuardResponse::Busy { request_id }
        }
        SNAPSHOT | EXITED => {
            if payload.len() < FRAGMENT_HEADER_LEN {
                return Err(ProtocolError::Truncated);
            }
            let final_fragment = match payload[4] {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::Invalid("invalid fragment final flag")),
            };
            return Ok(GuardResponsePart::Fragment {
                kind,
                request_id,
                sequence: read_u32(&payload[..4])?,
                final_fragment,
                data: payload[FRAGMENT_HEADER_LEN..].to_vec(),
            });
        }
        ERROR => GuardResponse::Error {
            request_id,
            message: String::from_utf8(payload.to_vec())?,
        },
        other => return Err(ProtocolError::UnknownKind(other)),
    };
    Ok(GuardResponsePart::Complete(response))
}

pub(super) fn decode_fragmented_response(
    kind: u8,
    request_id: u64,
    payload: &[u8],
) -> Result<GuardResponse, ProtocolError> {
    match kind {
        SNAPSHOT => {
            if payload.len() < 24 {
                return Err(ProtocolError::Truncated);
            }
            Ok(GuardResponse::Snapshot {
                request_id,
                offset: read_u64(&payload[0..8])?,
                total: read_u64(&payload[8..16])?,
                discarded: read_u64(&payload[16..24])?,
                data: payload[24..].to_vec(),
            })
        }
        EXITED if request_id == 0 => decode_exited(payload),
        EXITED => Err(ProtocolError::Invalid("Exited request id must be zero")),
        _ => Err(ProtocolError::Invalid("response kind cannot be fragmented")),
    }
}

fn decode_exited(payload: &[u8]) -> Result<GuardResponse, ProtocolError> {
    if payload.len() < 12 {
        return Err(ProtocolError::Truncated);
    }
    let metadata_len = read_u32(&payload[0..4])? as usize;
    let stdout_len = read_u32(&payload[4..8])? as usize;
    let stderr_len = read_u32(&payload[8..12])? as usize;
    let metadata_end = 12usize
        .checked_add(metadata_len)
        .ok_or(ProtocolError::Invalid("exited frame length overflow"))?;
    let stdout_end = metadata_end
        .checked_add(stdout_len)
        .ok_or(ProtocolError::Invalid("exited frame length overflow"))?;
    let stderr_end = stdout_end
        .checked_add(stderr_len)
        .ok_or(ProtocolError::Invalid("exited frame length overflow"))?;
    if stderr_end != payload.len() {
        return Err(ProtocolError::Truncated);
    }
    Ok(GuardResponse::Exited {
        exit: serde_json::from_slice(&payload[12..metadata_end])?,
        stdout: payload[metadata_end..stdout_end].to_vec(),
        stderr: payload[stdout_end..stderr_end].to_vec(),
    })
}

fn encode_frame(kind: u8, request_id: u64, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let body_len =
        FRAME_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(ProtocolError::FrameTooLarge {
                size: usize::MAX,
                max: MAX_FRAME_BODY,
            })?;
    ensure_frame_size(body_len)?;
    let body_len_u32 = usize_to_u32(body_len)?;
    let mut frame = Vec::with_capacity(4 + body_len);
    frame.extend_from_slice(&body_len_u32.to_be_bytes());
    frame.push(kind);
    frame.extend_from_slice(&request_id.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

fn split_body(body: &[u8]) -> Result<(u8, u64, &[u8]), ProtocolError> {
    if body.len() < FRAME_HEADER_LEN {
        return Err(ProtocolError::Truncated);
    }
    Ok((body[0], read_u64(&body[1..9])?, &body[9..]))
}

fn ensure_frame_size(size: usize) -> Result<(), ProtocolError> {
    if size > MAX_FRAME_BODY {
        return Err(ProtocolError::FrameTooLarge {
            size,
            max: MAX_FRAME_BODY,
        });
    }
    if size < FRAME_HEADER_LEN {
        return Err(ProtocolError::Truncated);
    }
    Ok(())
}

fn require_len(payload: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(ProtocolError::Invalid("unexpected payload length"))
    }
}

fn read_u32(bytes: &[u8]) -> Result<u32, ProtocolError> {
    Ok(u32::from_be_bytes(
        bytes.try_into().map_err(|_| ProtocolError::Truncated)?,
    ))
}

fn read_u64(bytes: &[u8]) -> Result<u64, ProtocolError> {
    Ok(u64::from_be_bytes(
        bytes.try_into().map_err(|_| ProtocolError::Truncated)?,
    ))
}

fn usize_to_u32(value: usize) -> Result<u32, ProtocolError> {
    u32::try_from(value).map_err(|_| ProtocolError::FrameTooLarge {
        size: value,
        max: MAX_FRAME_BODY,
    })
}

fn usize_to_u64(value: usize) -> Result<u64, ProtocolError> {
    u64::try_from(value).map_err(|_| ProtocolError::Invalid("usize does not fit u64"))
}

fn u64_to_usize(value: u64) -> Result<usize, ProtocolError> {
    usize::try_from(value).map_err(|_| ProtocolError::Invalid("u64 does not fit usize"))
}

#[cfg(test)]
fn encode_request_for_test(request: &GuardRequest) -> Result<Vec<u8>, ProtocolError> {
    encode_request(request)
}

#[cfg(test)]
fn decode_request_for_test(frame: &[u8]) -> Result<GuardRequest, ProtocolError> {
    if frame.len() < 4 {
        return Err(ProtocolError::Truncated);
    }
    let size = read_u32(&frame[0..4])? as usize;
    ensure_frame_size(size)?;
    if frame.len() != size + 4 {
        return Err(ProtocolError::Truncated);
    }
    decode_request_body(&frame[4..])
}

#[cfg(test)]
fn oversized_frame() -> Vec<u8> {
    u32::try_from(MAX_FRAME_BODY + 1)
        .unwrap()
        .to_be_bytes()
        .to_vec()
}

#[cfg(test)]
fn unknown_frame() -> Vec<u8> {
    let mut frame = Vec::with_capacity(13);
    frame.extend_from_slice(&u32::try_from(FRAME_HEADER_LEN).unwrap().to_be_bytes());
    frame.push(0xff);
    frame.extend_from_slice(&0u64.to_be_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use futures::StreamExt as _;

    use super::*;

    #[tokio::test]
    async fn request_stream_preserves_partial_frame_across_cancelled_poll() {
        let expected = GuardRequest::Read {
            request_id: 7,
            offset: 11,
            max_bytes: 13,
        };
        let frame = encode_request_for_test(&expected).expect("encode request");
        let (mut writer, reader) = tokio::io::duplex(frame.len());
        let mut requests = Box::pin(request_stream(reader));

        writer
            .write_all(&frame[..4])
            .await
            .expect("write frame length");
        {
            let next = requests.next();
            tokio::pin!(next);
            tokio::select! {
                result = &mut next => panic!("partial frame completed unexpectedly: {result:?}"),
                () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            }
        }
        writer
            .write_all(&frame[4..])
            .await
            .expect("write frame body");

        let actual = tokio::time::timeout(std::time::Duration::from_secs(1), requests.next())
            .await
            .expect("request stream timed out")
            .expect("request stream ended")
            .expect("decode request");
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_and_exited_frames_are_ordered_and_final() {
        let responses = [
            GuardResponse::Snapshot {
                request_id: 41,
                offset: 7,
                total: 9,
                discarded: 2,
                data: vec![b's'; MAX_FRAME_BODY + 17],
            },
            GuardResponse::Exited {
                exit: GuardExit {
                    status: super::super::status::GuardStatusKind::Completed,
                    exit_code: Some(0),
                    signal: None,
                    resource_limit: None,
                    omitted_output_bytes: 0,
                    omitted_log_bytes: 0,
                },
                stdout: vec![b'o'; MAX_FRAME_BODY + 17],
                stderr: vec![b'e'; MAX_FRAME_BODY + 17],
            },
        ];

        for response in responses {
            let (expected_kind, expected_request_id) = match &response {
                GuardResponse::Snapshot { request_id, .. } => (SNAPSHOT, *request_id),
                GuardResponse::Exited { .. } => (EXITED, 0),
                _ => unreachable!(),
            };
            let frames = encode_response_frames(&response).unwrap();
            assert!(frames.len() > 1);
            for (sequence, frame) in frames.iter().enumerate() {
                let body_len = read_u32(&frame[..4]).unwrap() as usize;
                assert!(body_len <= MAX_FRAME_BODY);
                let (kind, request_id, payload) = split_body(&frame[4..]).unwrap();
                assert_eq!(kind, expected_kind);
                assert_eq!(request_id, expected_request_id);
                assert_eq!(read_u32(&payload[..4]).unwrap() as usize, sequence);
                assert_eq!(payload[4], u8::from(sequence + 1 == frames.len()));
            }
        }
    }

    #[test]
    fn codec_round_trips_raw_terminal_bytes_and_request_id() {
        let request = GuardRequest::Write {
            request_id: 7,
            data: vec![0, 0xff, b'\n'],
        };
        let bytes = encode_request_for_test(&request).unwrap();
        assert_eq!(decode_request_for_test(&bytes).unwrap(), request);
    }

    #[test]
    fn codec_rejects_oversized_and_unknown_frames() {
        assert!(matches!(
            decode_request_for_test(&oversized_frame()),
            Err(ProtocolError::FrameTooLarge { .. })
        ));
        assert!(matches!(
            decode_request_for_test(&unknown_frame()),
            Err(ProtocolError::UnknownKind(0xff))
        ));
    }

    #[test]
    fn decoder_rejects_oversized_terminal_write() {
        let frame = encode_frame(WRITE, 7, &vec![0; MAX_TERMINAL_WRITE + 1]).unwrap();

        assert!(matches!(
            decode_request_for_test(&frame),
            Err(ProtocolError::FrameTooLarge { size, max })
                if size == MAX_TERMINAL_WRITE + 1 && max == MAX_TERMINAL_WRITE
        ));
    }

    #[test]
    fn guard_start_round_trips_without_deadline() {
        let request = GuardRequest::Start {
            request_id: 1,
            request: StartRequest {
                task_id: "task-1".to_owned(),
                kind: GuardTaskKind::Bash,
                command: "printf ready".to_owned(),
                limits: GuardLimits {
                    timeout_ms: None,
                    max_command_parallelism: 4,
                    max_command_descendant_processes: 32,
                    max_command_memory_percent: 25,
                    max_output_bytes: 65_536,
                    max_background_log_bytes: 10_485_760,
                },
                status_dir: PathBuf::from("status"),
                cols: None,
                rows: None,
            },
        };
        let bytes = encode_request_for_test(&request).expect("encode");
        assert_eq!(decode_request_for_test(&bytes).expect("decode"), request);
        let GuardRequest::Start { request, .. } = request else {
            unreachable!("constructed Start request");
        };
        assert_eq!(request.limits.timeout_ms, None);
    }
}
