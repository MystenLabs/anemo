// Wire format

use crate::{
    types::{
        request::{RawRequestHeader, RequestHeader},
        response::{RawResponseHeader, ResponseHeader},
        Version,
    },
    Config, Request, Response, Result,
};
use anyhow::{anyhow, bail};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

const ANEMO: &[u8; 5] = b"anemo";

/// Maximum number of bytes to pre-allocate when decoding a frame.
const MAX_PREALLOCATION: usize = 1 << 20; // 1 MB

/// Default maximum frame length.
const DEFAULT_MAX_FRAME_LENGTH: usize = 8 * 1024 * 1024; // 8 MB

/// A length-delimited codec that uses the same wire format as tokio-util's
/// `LengthDelimitedCodec` (4-byte big-endian length prefix + data).
pub(crate) struct MessageFrameCodec {
    state: DecodeState,
    max_frame_length: usize,
}

#[derive(Debug, Clone, Copy)]
enum DecodeState {
    /// Waiting for the 4-byte length prefix.
    Head,
    /// Accumulating body bytes; stores the total expected frame length.
    Data(usize),
}

impl MessageFrameCodec {
    fn new(max_frame_length: usize) -> Self {
        Self {
            state: DecodeState::Head,
            max_frame_length,
        }
    }
}

impl Decoder for MessageFrameCodec {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<BytesMut>> {
        loop {
            match self.state {
                DecodeState::Head => {
                    if src.len() < 4 {
                        src.reserve(4 - src.len());
                        return Ok(None);
                    }

                    let len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
                    src.advance(4);

                    if len > self.max_frame_length {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "frame of length {} exceeds max frame length of {}",
                                len, self.max_frame_length,
                            ),
                        ));
                    }

                    if len == 0 {
                        return Ok(Some(BytesMut::new()));
                    }

                    // Only pre-allocate up to MAX_PREALLOCATION.
                    let to_reserve = len.min(MAX_PREALLOCATION);
                    src.reserve(to_reserve);

                    self.state = DecodeState::Data(len);
                }
                DecodeState::Data(frame_len) => {
                    if src.len() < frame_len {
                        // Reserve only up to MAX_PREALLOCATION beyond what we already have.
                        let remaining = frame_len - src.len();
                        let to_reserve = remaining.min(MAX_PREALLOCATION);
                        src.reserve(to_reserve);
                        return Ok(None);
                    }

                    self.state = DecodeState::Head;
                    return Ok(Some(src.split_to(frame_len)));
                }
            }
        }
    }
}

impl Encoder<Bytes> for MessageFrameCodec {
    type Error = io::Error;

    fn encode(&mut self, data: Bytes, dst: &mut BytesMut) -> io::Result<()> {
        let len = data.len();
        if len > self.max_frame_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "frame of length {} exceeds max frame length of {}",
                    len, self.max_frame_length,
                ),
            ));
        }

        dst.reserve(4 + len);
        dst.put_u32(len as u32);
        dst.extend_from_slice(&data);
        Ok(())
    }
}

/// Returns a fully configured message frame codec for writing/reading
/// serialized frames to/from a socket.
pub(crate) fn network_message_frame_codec(config: &Config) -> MessageFrameCodec {
    let max_frame_length = config.max_frame_size().unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    MessageFrameCodec::new(max_frame_length)
}

/// Anemo requires mTLS in order to ensure that both sides of the connections are authenticated by
/// the other. This is specifically required so that regardless of which peer initiates a
/// connection, both sides will be able to know the PeerId of the other side. One challenge with
/// this is that due to the ordering of how certs are exchanged, the client side may think the
/// connection is fully established when in reality the server may still reject the connection. To
/// handle this anemo has a very brief handshake, essentially an ACK, that is initiated by the
/// server side to inform the client that the server has finished establishing the connection.
///
/// Performing this small handshake will also enable the server side to make decisions about
/// whether to keep the connection based on things like the client side's PeerId.
pub(crate) async fn handshake(
    connection: crate::connection::Connection,
) -> Result<crate::connection::Connection> {
    match connection.origin() {
        crate::ConnectionOrigin::Inbound => {
            let mut send_stream = connection.open_uni().await?;
            write_version_frame(&mut send_stream, Version::V1).await?;
            send_stream.finish()?;
            send_stream.stopped().await?;
        }
        crate::ConnectionOrigin::Outbound => {
            let mut recv_stream = connection.accept_uni().await?;
            read_version_frame(&mut recv_stream).await?;
        }
    }
    Ok(connection)
}

pub(crate) async fn read_version_frame<T: AsyncRead + Unpin>(
    recv_stream: &mut T,
) -> Result<Version> {
    let mut buf: [u8; 8] = [0; 8];
    recv_stream.read_exact(&mut buf).await?;
    if &buf[0..=4] != ANEMO || buf[7] != 0 {
        bail!("Invalid Protocol Header");
    }
    let version_be_bytes = [buf[5], buf[6]];
    let version = u16::from_be_bytes(version_be_bytes);
    Version::new(version)
}

pub(crate) async fn write_version_frame<T: AsyncWrite + Unpin>(
    send_stream: &mut T,
    version: Version,
) -> Result<()> {
    let mut buf: [u8; 8] = [0; 8];
    buf[0..=4].copy_from_slice(ANEMO);
    buf[5..=6].copy_from_slice(&version.to_u16().to_be_bytes());

    send_stream.write_all(&buf).await?;

    Ok(())
}

pub(crate) async fn write_request<T: AsyncWrite + Unpin>(
    send_stream: &mut FramedWrite<T, MessageFrameCodec>,
    request: Request<Bytes>,
) -> Result<()> {
    // Write Version Frame
    write_version_frame(send_stream.get_mut(), request.version()).await?;

    let (parts, body) = request.into_parts();

    // Write Request Header
    let raw_header = RawRequestHeader::from_header(parts);
    let mut buf = BytesMut::new();
    bincode::serialize_into((&mut buf).writer(), &raw_header)
        .expect("serialization should not fail");
    send_stream.send(buf.freeze()).await?;

    // Write Body
    send_stream.send(body).await?;

    Ok(())
}

pub(crate) async fn write_response<T: AsyncWrite + Unpin>(
    send_stream: &mut FramedWrite<T, MessageFrameCodec>,
    response: Response<Bytes>,
) -> Result<()> {
    // Write Version Frame
    write_version_frame(send_stream.get_mut(), response.version()).await?;

    // We keep extensions alive so that any RAII objects contained therein
    // are not dropped until the response is sent.
    let (parts, body) = response.into_parts();
    let (raw_header, _extensions) = RawResponseHeader::from_header(parts);

    // Write Request Header
    let mut buf = BytesMut::new();
    bincode::serialize_into((&mut buf).writer(), &raw_header)
        .expect("serialization should not fail");
    send_stream.send(buf.freeze()).await?;

    // Write Body
    send_stream.send(body).await?;

    Ok(())
}

pub(crate) async fn read_request<T: AsyncRead + Unpin>(
    recv_stream: &mut FramedRead<T, MessageFrameCodec>,
) -> Result<Request<Bytes>> {
    // Read Version Frame
    let version = read_version_frame(recv_stream.get_mut()).await?;

    // Read Request Header
    let header_buf = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;
    let raw_header: RawRequestHeader = bincode::deserialize(&header_buf)?;
    let request_header = RequestHeader::from_raw(raw_header, version);

    // Read Body
    let body = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;

    let request = Request::from_parts(request_header, body.freeze());

    Ok(request)
}

pub(crate) async fn read_response<T: AsyncRead + Unpin>(
    recv_stream: &mut FramedRead<T, MessageFrameCodec>,
) -> Result<Response<Bytes>> {
    // Read Version Frame
    let version = read_version_frame(recv_stream.get_mut()).await?;

    // Read Request Header
    let header_buf = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;
    let raw_header: RawResponseHeader = bincode::deserialize(&header_buf)?;
    let response_header = ResponseHeader::from_raw(raw_header, version)?;

    // Read Body
    let body = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;

    let response = Response::from_parts(response_header, body.freeze());

    Ok(response)
}

#[cfg(test)]
mod test {
    use super::{read_version_frame, write_version_frame, Version};

    const HEADER: [u8; 8] = [b'a', b'n', b'e', b'm', b'o', 0, 1, 0];

    #[tokio::test]
    async fn read_version_header() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&HEADER);

        let version = read_version_frame(&mut buf.as_ref()).await.unwrap();
        assert_eq!(Version::V1, version);
    }

    #[tokio::test]
    async fn read_incorrect_version_header() {
        // ANEMO header incorrect
        let header = [b'h', b't', b't', b'p', b'3', 0, 1, 0];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();

        // Reserved byte not 0
        let header = [b'a', b'n', b'e', b'm', b'o', 0, 1, 1];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();

        // Version is not 1
        let header = [b'a', b'n', b'e', b'm', b'o', 1, 0, 0];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();
    }

    #[tokio::test]
    async fn write_version_header() {
        let mut buf = Vec::new();

        write_version_frame(&mut buf, Version::V1).await.unwrap();
        assert_eq!(HEADER.as_ref(), buf);
    }
}

#[cfg(test)]
mod message_frame_codec_tests {
    use super::{MessageFrameCodec, DEFAULT_MAX_FRAME_LENGTH, MAX_PREALLOCATION};
    use bytes::{BufMut, Bytes, BytesMut};
    use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

    fn new_legacy_codec() -> LengthDelimitedCodec {
        LengthDelimitedCodec::builder()
            .length_field_length(4)
            .big_endian()
            .new_codec()
    }

    fn legacy_encode(codec: &mut LengthDelimitedCodec, data: &[u8]) -> BytesMut {
        let mut buf = BytesMut::new();
        codec
            .encode(Bytes::copy_from_slice(data), &mut buf)
            .unwrap();
        buf
    }

    fn custom_encode(codec: &mut MessageFrameCodec, data: &[u8]) -> BytesMut {
        let mut buf = BytesMut::new();
        codec
            .encode(Bytes::copy_from_slice(data), &mut buf)
            .unwrap();
        buf
    }

    #[test]
    fn empty_frame_legacy_to_custom() {
        let mut enc = new_legacy_codec();
        let wire = legacy_encode(&mut enc, &[]);

        let mut dec = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert!(frame.is_empty());
    }

    #[test]
    fn empty_frame_custom_to_legacy() {
        let mut enc = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let wire = custom_encode(&mut enc, &[]);

        let mut dec = new_legacy_codec();
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert!(frame.is_empty());
    }

    #[test]
    fn small_frame_legacy_to_custom() {
        let data: Vec<u8> = (0..64).collect();
        let mut enc = new_legacy_codec();
        let wire = legacy_encode(&mut enc, &data);

        let mut dec = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert_eq!(&frame[..], &data);
    }

    #[test]
    fn small_frame_custom_to_legacy() {
        let data: Vec<u8> = (0..64).collect();
        let mut enc = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let wire = custom_encode(&mut enc, &data);

        let mut dec = new_legacy_codec();
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert_eq!(&frame[..], &data);
    }

    #[test]
    fn medium_frame_around_preallocation_boundary() {
        let size = MAX_PREALLOCATION + 1;
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();

        // Legacy encode -> custom decode
        let mut enc = new_legacy_codec();
        let wire = legacy_encode(&mut enc, &data);
        let mut dec = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert_eq!(&frame[..], &data);

        // Custom encode -> legacy decode
        let mut enc = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let wire = custom_encode(&mut enc, &data);
        let mut dec = new_legacy_codec();
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert_eq!(&frame[..], &data);
    }

    #[test]
    fn large_frame_above_preallocation_limit() {
        let size = MAX_PREALLOCATION * 3 + 1;
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();

        // Encode with custom codec, decode with custom codec (full buffer available)
        let mut enc = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let wire = custom_encode(&mut enc, &data);
        let mut dec = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert_eq!(frame.len(), size);
        assert_eq!(&frame[..], &data);

        // Cross-codec: legacy encode -> custom decode
        let mut enc = new_legacy_codec();
        let wire = legacy_encode(&mut enc, &data);
        let mut dec = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert_eq!(&frame[..], &data);

        // Cross-codec: custom encode -> legacy decode
        let mut enc = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let wire = custom_encode(&mut enc, &data);
        let mut dec = new_legacy_codec();
        let frame = dec.decode(&mut wire.clone()).unwrap().unwrap();
        assert_eq!(&frame[..], &data);
    }

    #[test]
    fn max_frame_length_rejection_custom() {
        let max = 128;
        let data = vec![0u8; max + 1];

        // Encode should fail
        let mut enc = MessageFrameCodec::new(max);
        let mut buf = BytesMut::new();
        assert!(enc.encode(Bytes::from(data.clone()), &mut buf).is_err());

        // Decode should fail when length prefix exceeds max
        let mut dec = MessageFrameCodec::new(max);
        let mut wire = BytesMut::new();
        wire.put_u32((max + 1) as u32);
        assert!(dec.decode(&mut wire).is_err());
    }

    #[test]
    fn max_frame_length_rejection_matches_legacy() {
        let max = 128;
        let data = vec![0u8; max + 1];

        let mut legacy = LengthDelimitedCodec::builder()
            .length_field_length(4)
            .big_endian()
            .max_frame_length(max)
            .new_codec();
        let mut buf = BytesMut::new();
        let legacy_result = legacy.encode(Bytes::from(data.clone()), &mut buf);

        let mut custom = MessageFrameCodec::new(max);
        let mut buf = BytesMut::new();
        let custom_result = custom.encode(Bytes::from(data), &mut buf);

        // Both should reject oversized frames
        assert!(legacy_result.is_err());
        assert!(custom_result.is_err());
    }

    #[test]
    fn incremental_delivery_one_byte_at_a_time() {
        let data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let mut enc = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let wire = custom_encode(&mut enc, &data);

        let mut dec = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let mut src = BytesMut::new();

        // Feed one byte at a time; should return None until complete
        for i in 0..wire.len() - 1 {
            src.extend_from_slice(&wire[i..i + 1]);
            assert!(
                dec.decode(&mut src).unwrap().is_none(),
                "should not produce frame at byte {}",
                i
            );
        }

        // Feed the last byte
        src.extend_from_slice(&wire[wire.len() - 1..]);
        let frame = dec.decode(&mut src).unwrap().unwrap();
        assert_eq!(&frame[..], &data);
    }

    #[test]
    fn multiple_frames_in_sequence() {
        let data1: Vec<u8> = (0..100).collect();
        let data2: Vec<u8> = (100..250).collect();

        let mut enc = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let mut wire = BytesMut::new();
        enc.encode(Bytes::from(data1.clone()), &mut wire).unwrap();
        enc.encode(Bytes::from(data2.clone()), &mut wire).unwrap();

        let mut dec = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let frame1 = dec.decode(&mut wire).unwrap().unwrap();
        assert_eq!(&frame1[..], &data1);
        let frame2 = dec.decode(&mut wire).unwrap().unwrap();
        assert_eq!(&frame2[..], &data2);
    }

    #[test]
    fn preallocation_cap() {
        // Create a frame with a large claimed length
        let claimed_len: usize = 64 * 1024 * 1024; // 64 MB
        let mut wire = BytesMut::new();
        wire.put_u32(claimed_len as u32);
        // Only provide a few bytes of actual data — not the full frame
        wire.extend_from_slice(&[0u8; 64]);

        let mut dec = MessageFrameCodec::new(claimed_len);
        // Decode should return None (not enough data)
        assert!(dec.decode(&mut wire).unwrap().is_none());

        // The buffer capacity should be bounded to at most MAX_PREALLOCATION plus some overhead.
        assert!(
            wire.capacity() < MAX_PREALLOCATION * 2,
            "buffer capacity {} should be bounded",
            wire.capacity(),
        );
    }

    #[test]
    fn wire_format_compatibility() {
        // Verify the wire format is identical between legacy and custom codecs
        let data = b"hello world";

        let mut legacy = new_legacy_codec();
        let legacy_wire = legacy_encode(&mut legacy, data);

        let mut custom = MessageFrameCodec::new(DEFAULT_MAX_FRAME_LENGTH);
        let custom_wire = custom_encode(&mut custom, data);

        assert_eq!(legacy_wire, custom_wire);
    }
}
