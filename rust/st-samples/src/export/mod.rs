//! Export: which file formats exist ([`ExportFormat`]), the intermediate render file
//! ([`RenderFileWriter`]), delivery (sample-rate conversion, dither and encoding of a finished
//! render: [`DeliveryJob`]) and the filesystem transaction that publishes the result atomically
//! ([`ExportTransaction`]).
//!
//! The order of an export:
//!
//! 1. [`ExportFormat::resolve`] validates the request before anything is rendered.
//! 2. [`ExportTransaction::new`] reserves a private staging folder next to the destination and
//!    [`ExportTransaction::register`] names each output.
//! 3. The renderer (st-audio) writes each output's float render with [`RenderFileWriter`] to
//!    [`ExportEntry::source_path`].
//! 4. A [`DeliveryJob`] per output converts the render to [`ExportEntry::delivery_path`].
//! 5. [`ExportTransaction::publish`] makes everything visible at once; on any failure or
//!    cancellation [`ExportTransaction::discard`] removes the staging folder and the
//!    destination is as it was.

pub mod caf;
pub mod delivery;
pub mod format;
mod fsx;
#[cfg(unix)]
pub mod transaction;

pub use caf::{RenderFileReader, RenderFileWriter};
pub use delivery::{DeliveryJob, DeliveryProgress, DeliveryResult, DeliverySettings};
pub use format::{ExportCapabilities, ExportEncoding, ExportFormat, ExportFormatRequest, ExportMetadata, ExportMode};
#[cfg(unix)]
pub use transaction::{ExportEntry, ExportKind, ExportTransaction};
