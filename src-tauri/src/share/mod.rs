//! Sharing a fight with others. Each saved fight keeps a small slice of its
//! traffic beside it; the player can upload that slice to a2tools.app, or
//! write a Preview upload to disk to see what would go. A packet log can also
//! be copied for a bug report with every name blinded. Of all of this, only
//! `upload` ever opens a socket.

pub mod report;
pub mod report_debug;
pub mod report_log;
pub mod ring;

mod auto_upload;
mod preview;
mod slices;
mod upload;

pub use auto_upload::{auto_upload_retries_due, note_auto_upload_failure, wants_auto_upload, AUTO_UPLOAD_KEY};
pub use preview::{capture_files, load_capture_file};
pub use slices::{
    delete_slice_files, gzip, names_from, trim_old_slices, read_slice, store_slice, share_status, slice_stamp, slice_uploader, slice_folder,
    uploader_in, write_slice, SliceSidecar, SliceState,
};
pub use upload::{preview_upload, request_body, upload, upload_detailed, PublishError, UploadPreview, PublishOutcome};
