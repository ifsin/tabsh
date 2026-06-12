use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf8Error {
    valid_up_to: usize,
    error_len: Option<usize>,
}

impl Utf8Error {
    #[inline]
    pub fn valid_up_to(&self) -> usize {
        self.valid_up_to
    }

    #[inline]
    pub fn error_len(&self) -> Option<usize> {
        self.error_len
    }
}

impl fmt::Display for Utf8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.error_len {
            Some(len) => write!(
                f,
                "invalid utf-8 sequence of {} bytes from index {}",
                len, self.valid_up_to
            ),
            None => write!(
                f,
                "incomplete utf-8 byte sequence from index {}",
                self.valid_up_to
            ),
        }
    }
}

impl std::error::Error for Utf8Error {}

fn std_err_to_ours(e: std::str::Utf8Error) -> Utf8Error {
    Utf8Error {
        valid_up_to: e.valid_up_to(),
        error_len: e.error_len(),
    }
}

#[inline]
pub fn validate(bytes: &[u8]) -> Result<&str, Utf8Error> {
    std::str::from_utf8(bytes).map_err(std_err_to_ours)
}

#[inline]
pub fn from_utf8_fast(bytes: &[u8]) -> Result<&str, Utf8Error> {
    validate(bytes)
}

#[inline]
pub fn from_utf8_compat(bytes: &[u8]) -> Result<&str, Utf8Error> {
    validate(bytes)
}

#[inline]
pub fn from_utf8_to_string(bytes: &[u8]) -> Result<String, Utf8Error> {
    validate(bytes).map(|s| s.to_string())
}

#[inline]
pub fn from_utf8_lossy_fast(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}
