//! Storage engine subsystem: Pager, Slotted Page layout, and Block I/O.

pub mod page;
pub mod pager;

pub use page::{Page, PageType, PAGE_SIZE};
pub use pager::Pager;

