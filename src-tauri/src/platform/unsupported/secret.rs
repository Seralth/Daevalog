/// No per-user secret store here. Refuse rather than store a token in the
/// clear: an account that will not stay signed in beats a leaked credential.
pub fn available() -> bool {
    false
}

pub fn forget(_sealed: &[u8]) {}

pub fn protect(_plaintext: &[u8], _entropy: &[u8]) -> Option<Vec<u8>> {
    None
}

pub fn unprotect(
    _ciphertext: &[u8],
    _entropy: &[u8],
) -> Result<Vec<u8>, crate::platform::UnsealError> {
    Err(crate::platform::UnsealError::Unavailable)
}
