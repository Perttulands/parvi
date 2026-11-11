//! Work as other areas reach it (ARCHITECTURE.md, Provided interfaces):
//! the canonical reference for a written one, placed by this node's Work
//! configuration. The work area provides it; the rest name only this trait.

use std::future::Future;
use std::pin::Pin;

use parvi_core::Error;
use parvi_work_ref::WorkRef;

pub type Resolving<'a> = Pin<Box<dyn Future<Output = Result<WorkRef, Error>> + Send + 'a>>;

pub trait Resolver: Send + Sync {
    /// The canonical reference for `written` in any written form. A bare
    /// Bead id resolves only when exactly one configured store has its
    /// prefix; otherwise refused as [`crate::BAD_REQUEST`], saying why.
    fn resolve<'a>(&'a self, written: &'a str) -> Resolving<'a>;
}
