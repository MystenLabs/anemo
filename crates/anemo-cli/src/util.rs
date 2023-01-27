use std::convert::Infallible;

use anemo::{Request, Response};
use bytes::Bytes;

pub(crate) async fn noop_handle(_request: Request<Bytes>) -> Result<Response<Bytes>, Infallible> {
    Ok(Response::new(bytes::Bytes::new()))
}

pub fn random_key() -> [u8; 32] {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);
    bytes
}
