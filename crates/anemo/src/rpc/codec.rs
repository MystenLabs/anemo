use crate::error::BoxError;

pub use self::bincode::BincodeCodec;
pub use identity::IdentityCodec;
pub use json::JsonCodec;

/// Trait that knows how to encode and decode RPC messages.
pub trait Codec {
    /// The encodable message.
    type Encode: Send + 'static;
    /// The decodable message.
    type Decode: Send + 'static;

    /// The encoder that can encode a message.
    type Encoder: Encoder<Item = Self::Encode> + Send + 'static;
    /// The encoder that can decode a message.
    type Decoder: Decoder<Item = Self::Decode> + Send + 'static;

    /// Fetch the encoder.
    fn encoder(&mut self) -> Self::Encoder;
    /// Fetch the decoder.
    fn decoder(&mut self) -> Self::Decoder;

    /// Return the format name.
    fn format_name(&self) -> &'static str;
}

/// Encodes RPC message types
pub trait Encoder {
    /// The type that is encoded.
    type Item;

    /// The type of encoding errors.
    ///
    /// The type of unrecoverable frame encoding errors.
    type Error: Into<BoxError>;

    /// Encodes a message into the provided buffer.
    fn encode(&mut self, item: Self::Item) -> Result<bytes::Bytes, Self::Error>;
}

/// Decodes RPC message types
pub trait Decoder {
    /// The type that is decoded.
    type Item;

    /// The type of unrecoverable frame decoding errors.
    type Error: Into<BoxError>;

    /// Decode a message from the buffer.
    ///
    /// The buffer will contain exactly the bytes of a full message.
    fn decode(&mut self, src: bytes::Bytes) -> Result<Self::Item, Self::Error>;
}

mod json {
    use super::{Codec, Decoder, Encoder};
    use std::marker::PhantomData;

    #[derive(Debug)]
    pub struct JsonEncoder<T>(PhantomData<T>);

    impl<T: serde::Serialize> Encoder for JsonEncoder<T> {
        type Item = T;
        type Error = serde_json::Error;

        fn encode(&mut self, item: Self::Item) -> Result<bytes::Bytes, Self::Error> {
            let buf = serde_json::to_vec(&item)?;
            Ok(buf.into())
        }
    }

    #[derive(Debug)]
    pub struct JsonDecoder<U>(PhantomData<U>);

    impl<U: serde::de::DeserializeOwned> Decoder for JsonDecoder<U> {
        type Item = U;
        type Error = serde_json::Error;

        fn decode(&mut self, buf: bytes::Bytes) -> Result<Self::Item, Self::Error> {
            serde_json::from_slice(&buf)
        }
    }

    /// A [`Codec`] that implements `json` encoding/decoding via the serde library.
    #[derive(Debug, Clone)]
    pub struct JsonCodec<T, U>(PhantomData<(T, U)>);

    impl<T, U> Default for JsonCodec<T, U> {
        fn default() -> Self {
            Self(PhantomData)
        }
    }

    impl<T, U> Codec for JsonCodec<T, U>
    where
        T: serde::Serialize + Send + 'static,
        U: serde::de::DeserializeOwned + Send + 'static,
    {
        type Encode = T;
        type Decode = U;
        type Encoder = JsonEncoder<T>;
        type Decoder = JsonDecoder<U>;

        fn encoder(&mut self) -> Self::Encoder {
            JsonEncoder(PhantomData)
        }

        fn decoder(&mut self) -> Self::Decoder {
            JsonDecoder(PhantomData)
        }

        fn format_name(&self) -> &'static str {
            "json"
        }
    }
}

mod bincode {
    use super::{Codec, Decoder, Encoder};
    use std::marker::PhantomData;

    #[derive(Debug)]
    pub struct BincodeEncoder<T>(PhantomData<T>);

    impl<T: serde::Serialize> Encoder for BincodeEncoder<T> {
        type Item = T;
        type Error = bincode::Error;

        fn encode(&mut self, item: Self::Item) -> Result<bytes::Bytes, Self::Error> {
            let buf = bincode::serialize(&item)?;
            Ok(buf.into())
        }
    }

    #[derive(Debug)]
    pub struct BincodeDecoder<U>(PhantomData<U>);

    impl<U: serde::de::DeserializeOwned> Decoder for BincodeDecoder<U> {
        type Item = U;
        type Error = bincode::Error;

        fn decode(&mut self, buf: bytes::Bytes) -> Result<Self::Item, Self::Error> {
            bincode::deserialize(&buf)
        }
    }

    /// A [`Codec`] that implements `bincode` encoding/decoding via the serde library.
    #[derive(Debug, Clone)]
    pub struct BincodeCodec<T, U>(PhantomData<(T, U)>);

    impl<T, U> Default for BincodeCodec<T, U> {
        fn default() -> Self {
            Self(PhantomData)
        }
    }

    impl<T, U> Codec for BincodeCodec<T, U>
    where
        T: serde::Serialize + Send + 'static,
        U: serde::de::DeserializeOwned + Send + 'static,
    {
        type Encode = T;
        type Decode = U;
        type Encoder = BincodeEncoder<T>;
        type Decoder = BincodeDecoder<U>;

        fn encoder(&mut self) -> Self::Encoder {
            BincodeEncoder(PhantomData)
        }

        fn decoder(&mut self) -> Self::Decoder {
            BincodeDecoder(PhantomData)
        }

        fn format_name(&self) -> &'static str {
            "bincode"
        }
    }
}

mod identity {
    use super::{Codec, Decoder, Encoder};
    use std::convert::Infallible;

    pub struct IdentityEncoder;

    impl Encoder for IdentityEncoder {
        type Item = bytes::Bytes;
        type Error = Infallible;

        fn encode(&mut self, item: Self::Item) -> Result<bytes::Bytes, Self::Error> {
            Ok(item)
        }
    }

    #[derive(Debug)]
    pub struct IdentityDecoder;

    impl Decoder for IdentityDecoder {
        type Item = bytes::Bytes;
        type Error = Infallible;

        fn decode(&mut self, buf: bytes::Bytes) -> Result<Self::Item, Self::Error> {
            Ok(buf)
        }
    }

    /// A [`Codec`] that does nothing.
    #[derive(Debug, Clone)]
    pub struct IdentityCodec {
        format_name: &'static str,
    }

    impl IdentityCodec {
        pub fn new(format_name: &'static str) -> Self {
            Self { format_name }
        }
    }

    impl Codec for IdentityCodec {
        type Encode = bytes::Bytes;
        type Decode = bytes::Bytes;
        type Encoder = IdentityEncoder;
        type Decoder = IdentityDecoder;

        fn encoder(&mut self) -> Self::Encoder {
            IdentityEncoder
        }

        fn decoder(&mut self) -> Self::Decoder {
            IdentityDecoder
        }

        fn format_name(&self) -> &'static str {
            self.format_name
        }
    }
}
