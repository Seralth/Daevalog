/// No per-user secret store here. Refuse rather than store a token in the
/// clear: an account that will not stay signed in beats a leaked credential.
pub const AVAILABLE: bool = false;

pub fn protect(_plaintext: &[u8], _entropy: &[u8]) -> Option<Vec<u8>> {
    None
}

pub fn unprotect(_ciphertext: &[u8], _entropy: &[u8]) -> Option<Vec<u8>> {
    None
}
