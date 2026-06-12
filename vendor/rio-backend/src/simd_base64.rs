use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;

#[inline]
pub fn decode(input: &[u8]) -> Option<Vec<u8>> {
    STANDARD.decode(input).ok()
}

#[inline]
pub fn decode_no_pad(input: &[u8]) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(input).ok()
}
