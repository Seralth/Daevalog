//! Preparing a fight for sharing — and, for now, only ever writing it to disk.
//!
//! Nothing here opens a socket. The dry run exists so that "we do not upload
//! your character names" is a claim you can check rather than one you have to
//! believe: it writes the exact two files an upload would send, into a folder it
//! then opens for you, and `a2t-inspect` reads them back.
//!
//! The artifacts are deliberately separate:
//!
//! - `<fight>.a2es` — the Evidence Slice, the packets themselves. See
//!   `capture::evidence_slice`.
//! - `<fight>.a2es.gz` — the same thing, gzipped: byte for byte what an upload
//!   would put on the wire. Both are written because the compressed one is what
//!   gets sent and the uncompressed one is what is easy to check, and making
//!   people choose between those would defeat the point. (`a2t-inspect` reads
//!   either.)
//! - `<fight>.upload.json` — the derived summary. **No field of it holds a
//!   character name**, not even your own; participants are identified by
//!   `sha256(dbid)`, and the server learns who that is only if you register the
//!   character yourself. There is a test asserting no name from the fight
//!   appears anywhere in the JSON.

mod auto_upload;
pub mod dev_logs;
mod envelope;
mod preview;
pub mod report_log;
pub mod ring;
mod slices;
mod upload;

pub use auto_upload::{auto_upload_retries_due, note_auto_upload_failure, wants_auto_upload, AUTO_UPLOAD_KEY};
pub use envelope::{account_ref, build_envelope, Evidence, Participant, UploadEnvelope};
pub use preview::{find_captures, gzip, preview, read_capture, slice_for, PreviewResult};
pub use slices::{
    forget_slice, names_from, prune_slices, read_slice, save_slice, share_status, slice_stamp, slice_uploader,
    slices_dir, uploader_in, write_slice, ShareStatus, SliceMeta,
};
#[cfg(all(test, feature = "desktop"))]
pub(crate) use upload::base64;
pub use upload::{upload, upload_detailed, UploadFailure, UploadResult};
